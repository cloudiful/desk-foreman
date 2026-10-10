//! Concurrency cases (issue 742 P3): concurrent admitted intents leave exactly
//! one durable operation and monotonic counters.

use super::helpers::{create_app_with_token, test_state};
use super::http::spawn_http;
use super::requests::{provision, replace, status, stop, wire_job_id};
use super::support::{
    LifecycleDb, bring_manager_online, create_environment, create_manager, ensure_ack, poll_job,
    report, stop_ack,
};

#[tokio::test]
async fn concurrent_provisions_admit_exactly_one() {
    let db = LifecycleDb::setup().await;
    let (_, app_token) = create_app_with_token(&db.pool, "conc-prov-app").await;
    let (_, mgr_token) = create_manager(&db.pool, "conc-prov-mgr").await;
    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    bring_manager_online(&base, &mgr_token).await;
    let env = create_environment(&base, &app_token, "conc-prov-user").await;

    let first_base = base.clone();
    let second_base = base.clone();
    let first_token = app_token.clone();
    let second_token = app_token.clone();
    let (first, second) = tokio::join!(
        async move { provision(&first_base, &first_token, env, 0, "conc-prov-a").await },
        async move { provision(&second_base, &second_token, env, 0, "conc-prov-b").await },
    );
    let admitted = [first.0, second.0]
        .iter()
        .filter(|code| **code == 200)
        .count();
    assert_eq!(admitted, 1, "exactly one provision must be admitted");

    let jobs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_environment_jobs \
         WHERE environment_id = $1 AND job_kind = 'ensure'",
    )
    .bind(env)
    .fetch_one(&db.pool)
    .await
    .expect("count ensure jobs");
    assert_eq!(jobs, 1, "only one durable ensure intent may exist");

    handle.abort();
    db.teardown().await;
}

#[tokio::test]
async fn concurrent_stops_admit_exactly_one_and_increment_once() {
    let db = LifecycleDb::setup().await;
    let (_, app_token) = create_app_with_token(&db.pool, "conc-stop-app").await;
    let (_, mgr_token) = create_manager(&db.pool, "conc-stop-mgr").await;
    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    bring_manager_online(&base, &mgr_token).await;
    let env = create_environment(&base, &app_token, "conc-stop-user").await;

    provision(&base, &app_token, env, 0, "conc-stop-prov").await;
    let job = poll_job(&base, &mgr_token).await;
    report(
        &base,
        &mgr_token,
        job["job_id"].as_str().unwrap(),
        true,
        Some(ensure_ack(env, 0)),
        None,
        204,
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "ready");

    let first_base = base.clone();
    let second_base = base.clone();
    let first_token = app_token.clone();
    let second_token = app_token.clone();
    let (first, second) = tokio::join!(
        async move { stop(&first_base, &first_token, env, 0, "conc-stop-a").await },
        async move { stop(&second_base, &second_token, env, 0, "conc-stop-b").await },
    );
    let admitted = [first.0, second.0]
        .iter()
        .filter(|code| **code == 200)
        .count();
    assert_eq!(admitted, 1, "exactly one stop must be admitted");

    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(
        desc["generation"], 1,
        "the authorization generation must increment exactly once"
    );

    let stop_jobs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_environment_jobs \
         WHERE environment_id = $1 AND job_kind = 'stop'",
    )
    .bind(env)
    .fetch_one(&db.pool)
    .await
    .expect("count stop jobs");
    assert_eq!(stop_jobs, 1, "only one durable stop intent may exist");

    // The single admitted stop still reconciles to a replacement.
    let job = poll_job(&base, &mgr_token).await;
    let stop_wire = job["job_id"].as_str().unwrap().to_string();
    report(
        &base,
        &mgr_token,
        &stop_wire,
        true,
        Some(stop_ack(env, 0, "exited")),
        None,
        204,
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "replaced");

    handle.abort();
    db.teardown().await;
}

#[tokio::test]
async fn concurrent_replacements_admit_exactly_one_and_consume_the_proof_once() {
    let db = LifecycleDb::setup().await;
    let (_, app_token) = create_app_with_token(&db.pool, "conc-repl-app").await;
    let (_, mgr_token) = create_manager(&db.pool, "conc-repl-mgr").await;
    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    bring_manager_online(&base, &mgr_token).await;
    let env = create_environment(&base, &app_token, "conc-repl-user").await;

    provision(&base, &app_token, env, 0, "conc-repl-prov").await;
    let job = poll_job(&base, &mgr_token).await;
    report(
        &base,
        &mgr_token,
        job["job_id"].as_str().unwrap(),
        true,
        Some(ensure_ack(env, 0)),
        None,
        204,
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "ready");

    let (_, desc) = stop(&base, &app_token, env, 0, "conc-repl-stop").await;
    let stop_job = desc["operation"]["job_id"].as_str().unwrap().to_string();
    poll_job(&base, &mgr_token).await;
    report(
        &base,
        &mgr_token,
        &wire_job_id(&stop_job),
        true,
        Some(stop_ack(env, 0, "exited")),
        None,
        204,
    )
    .await;
    let (_, desc) = status(&base, &app_token, env).await;
    assert_eq!(desc["state"], "replaced");

    // Two replacements race for the same single-use proof.
    let first_base = base.clone();
    let second_base = base.clone();
    let first_token = app_token.clone();
    let second_token = app_token.clone();
    let first_proof = stop_job.clone();
    let second_proof = stop_job.clone();
    let (first, second) = tokio::join!(
        async move {
            replace(
                &first_base,
                &first_token,
                env,
                1,
                0,
                &first_proof,
                "conc-repl-a",
            )
            .await
        },
        async move {
            replace(
                &second_base,
                &second_token,
                env,
                1,
                0,
                &second_proof,
                "conc-repl-b",
            )
            .await
        },
    );
    let admitted = [first.0, second.0]
        .iter()
        .filter(|code| **code == 200)
        .count();
    assert_eq!(admitted, 1, "exactly one replacement must be admitted");

    let ensure_jobs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_environment_jobs \
         WHERE environment_id = $1 AND job_kind = 'ensure' AND state = 'pending'",
    )
    .bind(env)
    .fetch_one(&db.pool)
    .await
    .expect("count pending ensure jobs");
    assert_eq!(ensure_jobs, 1, "only one replacement ensure may be pending");

    handle.abort();
    db.teardown().await;
}
