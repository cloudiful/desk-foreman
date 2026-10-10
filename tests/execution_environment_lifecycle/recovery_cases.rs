//! Recovery loop cases (issue 742 P3): an uncertain ensure plus an uncertain
//! stop recover through a new stop, a confirmed barrier ack supersedes the old
//! ensure, and a replacement then succeeds.

use super::helpers::{create_app_with_token, test_state};
use super::http::spawn_http;
use super::requests::{provision, replace, status, stop, wire_job_id};
use super::support::{
    LifecycleDb, bring_manager_online, create_environment, create_manager, ensure_ack, poll_job,
    report_failed, report_ok, stop_ack,
};

/// Regression for review 12084: a succeeded stop from an earlier generation is
/// historical proof and must never short-circuit a later stop for the current
/// candidate. After a full stop/replace cycle, a fresh stop must create a new
/// operation for the new candidate while the historical stop row is unchanged.
#[tokio::test]
async fn historical_stop_proof_does_not_short_circuit_a_new_candidate_stop() {
    let db = LifecycleDb::setup().await;
    let (_, app_token) = create_app_with_token(&db.pool, "hist-app").await;
    let (_, mgr_token) = create_manager(&db.pool, "hist-mgr").await;
    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    bring_manager_online(&base, &mgr_token).await;
    let env = create_environment(&base, &app_token, "hist-user").await;

    // Cycle 1 at candidate 0.
    provision(&base, &app_token, env, 0, "hist-prov-1").await;
    let job = poll_job(&base, &mgr_token).await;
    report_ok(
        &base,
        &mgr_token,
        job["job_id"].as_str().unwrap(),
        ensure_ack(env, 0),
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "ready");

    let (_, desc) = stop(&base, &app_token, env, 0, "hist-stop-1").await;
    let first_stop = desc["operation"]["job_id"].as_str().unwrap().to_string();
    poll_job(&base, &mgr_token).await;
    report_ok(
        &base,
        &mgr_token,
        &wire_job_id(&first_stop),
        stop_ack(env, 0, "exited"),
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "replaced");

    let (code, desc) = replace(&base, &app_token, env, 1, 0, &first_stop, "hist-repl-1").await;
    assert_eq!(code, 200, "first replace must be admitted: {desc}");
    let job = poll_job(&base, &mgr_token).await;
    report_ok(
        &base,
        &mgr_token,
        job["job_id"].as_str().unwrap(),
        ensure_ack(env, 1),
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "ready");
    assert_eq!(desc["generation"], 1);

    // A second stop must create a new operation for candidate 1, not replay the
    // historical succeeded stop.
    let (code, desc) = stop(&base, &app_token, env, 1, "hist-stop-2").await;
    assert_eq!(code, 200, "second stop must be admitted: {desc}");
    assert_eq!(desc["state"], "stopping");
    assert_eq!(desc["operation"]["kind"], "stop");
    let second_stop = desc["operation"]["job_id"].as_str().unwrap().to_string();
    assert_ne!(
        second_stop, first_stop,
        "a new stop must not reuse the historical proof operation"
    );

    let job = poll_job(&base, &mgr_token).await;
    assert_eq!(job["kind"], "stop_execution_environment");
    assert_eq!(job["job_id"].as_str().unwrap(), wire_job_id(&second_stop));
    assert_eq!(
        job["payload"]["owner"]["ExecutionEnvironment"]["generation"],
        1
    );

    // The historical stop row is untouched evidence, not repurposed.
    let first_state: String =
        sqlx::query_scalar("SELECT state FROM execution_environment_jobs WHERE job_id = $1")
            .bind(uuid::Uuid::parse_str(&first_stop).unwrap())
            .fetch_one(&db.pool)
            .await
            .expect("historical stop state");
    assert_eq!(first_state, "succeeded");

    report_ok(
        &base,
        &mgr_token,
        &wire_job_id(&second_stop),
        stop_ack(env, 1, "dead"),
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "replaced");
    assert_eq!(desc["generation"], 2);

    handle.abort();
    db.teardown().await;
}

#[tokio::test]
async fn uncertain_ensure_and_stop_recover_then_replace() {
    let db = LifecycleDb::setup().await;
    let (_, app_token) = create_app_with_token(&db.pool, "recovery-app").await;
    let (_, mgr_token) = create_manager(&db.pool, "recovery-mgr").await;
    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    bring_manager_online(&base, &mgr_token).await;
    let env = create_environment(&base, &app_token, "recovery-user").await;

    // Provision, then the ensure fails: the candidate is retained.
    let (_, desc) = provision(&base, &app_token, env, 0, "recovery-prov").await;
    let ensure_job = desc["operation"]["job_id"].as_str().unwrap().to_string();
    poll_job(&base, &mgr_token).await;
    report_failed(
        &base,
        &mgr_token,
        &wire_job_id(&ensure_job),
        "docker unavailable",
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(
        desc["generation"], 0,
        "uncertain ensure keeps the candidate"
    );
    assert_ne!(desc["state"], "ready");

    // First stop, then the stop fails too: state becomes uncertain.
    let (status_code, desc) = stop(&base, &app_token, env, 0, "recovery-stop-1").await;
    assert_eq!(status_code, 200, "stop must be admitted: {desc}");
    assert_eq!(desc["generation"], 1);
    let first_stop = desc["operation"]["job_id"].as_str().unwrap().to_string();
    poll_job(&base, &mgr_token).await;
    report_failed(
        &base,
        &mgr_token,
        &wire_job_id(&first_stop),
        "docker unavailable",
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "uncertain");
    assert_eq!(
        desc["generation"], 1,
        "uncertain stop does not re-increment"
    );

    // A recovery stop is admitted for the same candidate without another
    // generation increment.
    let (status_code, desc) = stop(&base, &app_token, env, 1, "recovery-stop-2").await;
    assert_eq!(status_code, 200, "recovery stop must be admitted: {desc}");
    assert_eq!(desc["state"], "stopping");
    assert_eq!(
        desc["generation"], 1,
        "recovery stop must not increment again"
    );
    let recovery_stop = desc["operation"]["job_id"].as_str().unwrap().to_string();

    let job = poll_job(&base, &mgr_token).await;
    assert_eq!(
        job["payload"]["owner"]["ExecutionEnvironment"]["generation"],
        0
    );
    report_ok(
        &base,
        &mgr_token,
        &wire_job_id(&recovery_stop),
        stop_ack(env, 0, "dead"),
    )
    .await;

    // Status records the proof, supersedes the old uncertain ensure, and marks
    // replaced; the obsolete ensure is never a success.
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "replaced");
    let superseded: String =
        sqlx::query_scalar("SELECT state FROM execution_environment_jobs WHERE job_id = $1")
            .bind(uuid::Uuid::parse_str(&ensure_job).unwrap())
            .fetch_one(&db.pool)
            .await
            .expect("superseded ensure state");
    assert_eq!(superseded, "superseded");

    // Replacement works at the next generation.
    let (status_code, desc) = replace(
        &base,
        &app_token,
        env,
        1,
        0,
        &recovery_stop,
        "recovery-replace",
    )
    .await;
    assert_eq!(status_code, 200, "replace must be admitted: {desc}");
    assert_eq!(desc["state"], "starting");
    assert_eq!(desc["generation"], 1);

    let job = poll_job(&base, &mgr_token).await;
    assert_eq!(
        job["payload"]["owner"]["ExecutionEnvironment"]["generation"],
        1
    );
    report_ok(
        &base,
        &mgr_token,
        job["job_id"].as_str().unwrap(),
        ensure_ack(env, 1),
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "ready");
    assert_eq!(desc["generation"], 1);

    handle.abort();
    db.teardown().await;
}

#[tokio::test]
async fn proof_is_consumed_once_and_cannot_be_reused() {
    let db = LifecycleDb::setup().await;
    let (_, app_token) = create_app_with_token(&db.pool, "proof-app").await;
    let (_, mgr_token) = create_manager(&db.pool, "proof-mgr").await;
    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    bring_manager_online(&base, &mgr_token).await;
    let env = create_environment(&base, &app_token, "proof-user").await;

    provision(&base, &app_token, env, 0, "proof-prov").await;
    let job = poll_job(&base, &mgr_token).await;
    report_ok(
        &base,
        &mgr_token,
        job["job_id"].as_str().unwrap(),
        ensure_ack(env, 0),
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "ready");

    let (_, desc) = stop(&base, &app_token, env, 0, "proof-stop").await;
    let stop_job = desc["operation"]["job_id"].as_str().unwrap().to_string();
    poll_job(&base, &mgr_token).await;
    report_ok(
        &base,
        &mgr_token,
        &wire_job_id(&stop_job),
        stop_ack(env, 0, "exited"),
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "replaced");

    let (status_code, _) = replace(&base, &app_token, env, 1, 0, &stop_job, "proof-replace").await;
    assert_eq!(status_code, 200, "first replace consumes the proof");

    // The same proof cannot be reused.
    let (status_code, body) =
        replace(&base, &app_token, env, 1, 1, &stop_job, "proof-replace-2").await;
    assert_eq!(status_code, 409, "a consumed proof must be refused: {body}");

    handle.abort();
    db.teardown().await;
}
