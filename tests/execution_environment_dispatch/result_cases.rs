//! Success and terminal-replay cases for durable result transport (issue 742
//! P2).

use super::ack::{RESULT, ensure_ack, pull, result_body, stop_ack};
use super::helpers::{create_app_with_token, test_state};
use super::http::{http_post, spawn_http};
use super::support::{
    DispatchDb, create_environment, create_manager_with_token, future_deadline, job_result,
    job_state, seed_job,
};

#[tokio::test]
async fn consistent_success_and_confirmed_stop_persist() {
    let db = DispatchDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "dispatch-ok-app").await;
    let (manager, token) = create_manager_with_token(&db.pool, "dispatch-ok-mgr").await;

    let ensure_env = create_environment(&db.pool, app, "ok-ensure").await;
    let ensure_id = seed_job(
        &db.pool,
        app,
        ensure_env,
        manager,
        "ensure",
        0,
        "ok-ensure-token",
        future_deadline(),
    )
    .await;
    let stop_env = create_environment(&db.pool, app, "ok-stop").await;
    let stop_id = seed_job(
        &db.pool,
        app,
        stop_env,
        manager,
        "stop",
        0,
        "ok-stop-token",
        future_deadline(),
    )
    .await;

    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;

    let pulled = pull(&base, &token).await;
    assert_eq!(pulled, format!("envjob:{ensure_id}"));
    let (status, _) = http_post(
        &base,
        RESULT,
        Some(&token),
        result_body(
            &pulled,
            true,
            Some(ensure_ack(ensure_env, 0, "running")),
            None,
        ),
    )
    .await;
    assert_eq!(status, 204, "a consistent ensure success must be recorded");
    assert_eq!(job_state(&db.pool, ensure_id).await, "succeeded");
    let stored = job_result(&db.pool, ensure_id)
        .await
        .expect("ensure result");
    assert_eq!(stored["outcome"], "ensure_running");
    assert_eq!(stored["observed_state"], "running");

    let pulled = pull(&base, &token).await;
    assert_eq!(pulled, format!("envjob:{stop_id}"));
    let (status, _) = http_post(
        &base,
        RESULT,
        Some(&token),
        result_body(
            &pulled,
            true,
            Some(stop_ack(stop_env, 0, true, "exited")),
            None,
        ),
    )
    .await;
    assert_eq!(status, 204, "a confirmed stop must be recorded");
    assert_eq!(job_state(&db.pool, stop_id).await, "succeeded");
    let stored = job_result(&db.pool, stop_id).await.expect("stop result");
    assert_eq!(stored["outcome"], "stop_confirmed");
    assert_eq!(stored["confirmed"], true);
    assert_eq!(stored["observed_state"], "exited");

    handle.abort();
    db.teardown().await;
}

#[tokio::test]
async fn duplicate_terminal_replay_is_idempotent_and_conflict_is_refused() {
    let db = DispatchDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "dispatch-replay-app").await;
    let (manager, token) = create_manager_with_token(&db.pool, "dispatch-replay-mgr").await;
    let env = create_environment(&db.pool, app, "replay-stop").await;
    let job_id = seed_job(
        &db.pool,
        app,
        env,
        manager,
        "stop",
        0,
        "replay-token",
        future_deadline(),
    )
    .await;

    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    let pulled = pull(&base, &token).await;
    assert_eq!(pulled, format!("envjob:{job_id}"));

    let first = result_body(&pulled, true, Some(stop_ack(env, 0, true, "exited")), None);
    let (status, _) = http_post(&base, RESULT, Some(&token), first.clone()).await;
    assert_eq!(status, 204);
    assert_eq!(job_state(&db.pool, job_id).await, "succeeded");

    // Identical replay preserves the recorded outcome.
    let (status, _) = http_post(&base, RESULT, Some(&token), first).await;
    assert_eq!(
        status, 204,
        "an identical terminal replay must be idempotent"
    );

    // A different consistent success contradicts the record.
    let (status, _) = http_post(
        &base,
        RESULT,
        Some(&token),
        result_body(&pulled, true, Some(stop_ack(env, 0, true, "absent")), None),
    )
    .await;
    assert_eq!(status, 409, "a conflicting replay must be refused");
    assert_eq!(job_state(&db.pool, job_id).await, "succeeded");
    let stored = job_result(&db.pool, job_id).await.expect("stop result");
    assert_eq!(stored["observed_state"], "exited");

    handle.abort();
    db.teardown().await;
}
