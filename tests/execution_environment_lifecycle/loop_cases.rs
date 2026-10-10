//! Full provision -> ensure -> ready -> stop -> replaced -> replace -> ready
//! loop, plus recovery and durability cases (issue 742 P3).

use super::helpers::{create_app_with_token, test_state};
use super::http::spawn_http;
use super::requests::{provision, replace, status, stop};
use super::support::{
    LifecycleDb, bring_manager_online, create_environment, create_manager, ensure_ack, poll_job,
    report_ok, stop_ack,
};

#[tokio::test]
async fn full_provision_stop_replace_loop() {
    let db = LifecycleDb::setup().await;
    let (_, app_token) = create_app_with_token(&db.pool, "loop-app").await;
    let (_, mgr_token) = create_manager(&db.pool, "loop-mgr").await;
    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    bring_manager_online(&base, &mgr_token).await;
    let env = create_environment(&base, &app_token, "loop-user").await;

    // Provision: admits one ensure intent at generation 0.
    let (status_code, desc) = provision(&base, &app_token, env, 0, "prov-1").await;
    assert_eq!(status_code, 200, "provision must be admitted: {desc}");
    assert_eq!(desc["state"], "starting");
    assert_eq!(desc["generation"], 0);
    assert_eq!(desc["operation"]["kind"], "ensure");
    assert_eq!(desc["operation"]["state"], "pending");
    assert!(
        desc.get("state_volume_ref").is_none() && desc.get("assigned_manager_id").is_none(),
        "descriptor must not expose internal refs: {desc}"
    );

    // Manager pulls and acknowledges the ensure.
    let job = poll_job(&base, &mgr_token).await;
    assert_eq!(job["kind"], "ensure_execution_environment");
    assert_eq!(
        job["payload"]["owner"]["ExecutionEnvironment"]["generation"],
        0
    );
    report_ok(
        &base,
        &mgr_token,
        job["job_id"].as_str().unwrap(),
        ensure_ack(env, 0),
    )
    .await;

    let (status_code, desc) = status(&base, &app_token, env).await;
    assert_eq!(status_code, 200);
    assert_eq!(desc["state"], "ready", "matching ensure must mark ready");
    assert_eq!(desc["generation"], 0);

    // Stop: increments the authorization generation once and keeps the
    // candidate at generation 0.
    let (status_code, desc) = stop(&base, &app_token, env, 0, "stop-1").await;
    assert_eq!(status_code, 200, "stop must be admitted: {desc}");
    assert_eq!(desc["state"], "stopping");
    assert_eq!(desc["generation"], 1, "first stop increments once");
    assert_eq!(desc["operation"]["kind"], "stop");
    let stop_job_id = desc["operation"]["job_id"].as_str().unwrap().to_string();

    let job = poll_job(&base, &mgr_token).await;
    assert_eq!(job["kind"], "stop_execution_environment");
    let stop_wire = job["job_id"].as_str().unwrap().to_string();
    assert_eq!(stop_wire, super::requests::wire_job_id(&stop_job_id));
    assert_eq!(
        job["payload"]["owner"]["ExecutionEnvironment"]["generation"],
        0
    );
    report_ok(&base, &mgr_token, &stop_wire, stop_ack(env, 0, "exited")).await;

    // Status records the proof and marks replaced.
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "replaced");
    assert_eq!(desc["generation"], 1);

    // Replace: consumes the proof and starts the next generation.
    let (status_code, desc) = replace(&base, &app_token, env, 1, 0, &stop_job_id, "repl-1").await;
    assert_eq!(status_code, 200, "replace must be admitted: {desc}");
    assert_eq!(desc["state"], "starting");
    assert_eq!(desc["generation"], 1);
    assert_eq!(desc["operation"]["kind"], "ensure");

    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "starting");

    let job = poll_job(&base, &mgr_token).await;
    assert_eq!(job["kind"], "ensure_execution_environment");
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
async fn late_ensure_cannot_revert_stopping_or_replaced_to_ready() {
    let db = LifecycleDb::setup().await;
    let (_, app_token) = create_app_with_token(&db.pool, "late-app").await;
    let (_, mgr_token) = create_manager(&db.pool, "late-mgr").await;
    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    bring_manager_online(&base, &mgr_token).await;
    let env = create_environment(&base, &app_token, "late-user").await;

    let (_, desc) = provision(&base, &app_token, env, 0, "late-prov").await;
    let ensure_job = desc["operation"]["job_id"].as_str().unwrap().to_string();
    poll_job(&base, &mgr_token).await;

    // Stop before the ensure is confirmed.
    let (status_code, desc) = stop(&base, &app_token, env, 0, "late-stop").await;
    assert_eq!(status_code, 200, "stop must be admitted: {desc}");
    assert_eq!(desc["state"], "stopping");
    let stop_job = desc["operation"]["job_id"].as_str().unwrap().to_string();

    // The old ensure finishes late; it must not revert stopping to ready.
    report_ok(
        &base,
        &mgr_token,
        &super::requests::wire_job_id(&ensure_job),
        ensure_ack(env, 0),
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_ne!(
        desc["state"], "ready",
        "a late ensure must not mark a stopping environment ready"
    );

    // Confirm the stop, then replay the late ensure again after replacement.
    poll_job(&base, &mgr_token).await;
    report_ok(
        &base,
        &mgr_token,
        &super::requests::wire_job_id(&stop_job),
        stop_ack(env, 0, "absent"),
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "replaced");
    report_ok(
        &base,
        &mgr_token,
        &super::requests::wire_job_id(&ensure_job),
        ensure_ack(env, 0),
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(
        desc["state"], "replaced",
        "a late ensure must not revert a replaced environment"
    );

    handle.abort();
    db.teardown().await;
}

#[tokio::test]
async fn affinity_candidate_and_proof_survive_pool_reopen() {
    let db = LifecycleDb::setup().await;
    let (_, app_token) = create_app_with_token(&db.pool, "reopen-app").await;
    let (manager_id, mgr_token) = create_manager(&db.pool, "reopen-mgr").await;
    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    bring_manager_online(&base, &mgr_token).await;
    let env = create_environment(&base, &app_token, "reopen-user").await;

    provision(&base, &app_token, env, 0, "reopen-prov").await;
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

    // Reopen the pool and read durable state through the same route.
    let reopened = db.reconnect().await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "ready");
    assert_eq!(desc["generation"], 0);

    // The binding and candidate survived; stop still works after reopen.
    let (status_code, _) = stop(&base, &app_token, env, 0, "reopen-stop").await;
    assert_eq!(status_code, 200, "stop must still admit after reopen");

    let bound: Option<i64> = sqlx::query_scalar(
        "SELECT assigned_manager_id FROM execution_environments WHERE environment_id = $1",
    )
    .bind(env)
    .fetch_one(&reopened)
    .await
    .expect("read binding");
    assert_eq!(bound, Some(manager_id), "affinity survives reopen");

    handle.abort();
    db.teardown().await;
}

#[tokio::test]
async fn offline_manager_is_never_reassigned() {
    let db = LifecycleDb::setup().await;
    let (_, app_token) = create_app_with_token(&db.pool, "offline-app").await;
    let (manager_a, _) = create_manager(&db.pool, "offline-mgr-a").await;
    let (_, mgr_a_token) = create_manager(&db.pool, "offline-mgr-a2").await;
    let (_, mgr_b_token) = create_manager(&db.pool, "offline-mgr-b").await;
    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    bring_manager_online(&base, &mgr_a_token).await;
    bring_manager_online(&base, &mgr_b_token).await;

    let env = create_environment(&base, &app_token, "offline-user").await;

    // No binding yet and only offline managers: provision is unavailable and
    // binds nothing.
    super::support::set_manager_online(&db.pool, manager_a, false).await;
    // Disable the other manager too.
    sqlx::query("UPDATE runner_managers SET status='offline', last_seen_at=NULL")
        .execute(&db.pool)
        .await
        .expect("offline all managers");
    let (status_code, body) = provision(&base, &app_token, env, 0, "offline-prov").await;
    assert_eq!(
        status_code, 503,
        "no online manager must be unavailable: {body}"
    );
    let bound: Option<i64> = sqlx::query_scalar(
        "SELECT assigned_manager_id FROM execution_environments WHERE environment_id = $1",
    )
    .bind(env)
    .fetch_one(&db.pool)
    .await
    .expect("read binding");
    assert!(
        bound.is_none(),
        "a failed provision must not bind a manager"
    );

    handle.abort();
    db.teardown().await;
}

#[tokio::test]
async fn unknown_environment_is_not_found() {
    let db = LifecycleDb::setup().await;
    let (_, app_token) = create_app_with_token(&db.pool, "missing-app").await;
    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;

    let missing = uuid::Uuid::new_v4();
    let (status_code, _) = provision(&base, &app_token, missing, 0, "missing-prov").await;
    assert_eq!(status_code, 404);
    let (status_code, _) = status(&base, &app_token, missing).await;
    assert_eq!(status_code, 404);

    handle.abort();
    db.teardown().await;
}
