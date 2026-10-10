//! Rejection and uncertainty cases for durable result transport (issue 742 P2).

use serde_json::json;

use super::ack::{RESULT, ensure_ack, owner, pull, result_body, stop_ack};
use super::helpers::{create_app_with_token, test_state};
use super::http::{http_post, spawn_http};
use super::support::{
    DispatchDb, create_environment, create_manager_with_token, future_deadline, job_error_summary,
    job_result, job_state, seed_job,
};

#[tokio::test]
async fn inconsistent_acks_are_rejected_and_leave_the_row_claimable() {
    let db = DispatchDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "dispatch-bad-app").await;
    let (manager, token) = create_manager_with_token(&db.pool, "dispatch-bad-mgr").await;
    let (_, other_token) = create_manager_with_token(&db.pool, "dispatch-bad-other").await;
    let env = create_environment(&db.pool, app, "bad-ack").await;
    let job_id = seed_job(
        &db.pool,
        app,
        env,
        manager,
        "ensure",
        3,
        "bad-token",
        future_deadline(),
    )
    .await;

    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    let pulled = pull(&base, &token).await;
    assert_eq!(pulled, format!("envjob:{job_id}"));
    assert_eq!(job_state(&db.pool, job_id).await, "claimed");

    // Another manager cannot complete this job.
    let (status, _) = http_post(
        &base,
        RESULT,
        Some(&other_token),
        result_body(&pulled, true, Some(ensure_ack(env, 3, "running")), None),
    )
    .await;
    assert_eq!(status, 403, "a foreign manager must be refused");
    assert_eq!(job_state(&db.pool, job_id).await, "claimed");

    // Wrong generation.
    let (status, _) = http_post(
        &base,
        RESULT,
        Some(&token),
        result_body(&pulled, true, Some(ensure_ack(env, 4, "running")), None),
    )
    .await;
    assert_eq!(status, 400, "a wrong-generation ack must be refused");

    // Wrong container name.
    let (status, _) = http_post(
        &base,
        RESULT,
        Some(&token),
        result_body(
            &pulled,
            true,
            Some(json!({
                "owner": owner(env, 3),
                "container_name": "not-our-container",
                "state": "running",
            })),
            None,
        ),
    )
    .await;
    assert_eq!(status, 400, "a wrong-container ack must be refused");

    // Non-running ensure ack.
    let (status, _) = http_post(
        &base,
        RESULT,
        Some(&token),
        result_body(&pulled, true, Some(ensure_ack(env, 3, "exited")), None),
    )
    .await;
    assert_eq!(status, 400, "a non-running ensure ack must be refused");

    // Malformed payload and an undiagnosed failure.
    let (status, _) = http_post(
        &base,
        RESULT,
        Some(&token),
        result_body(&pulled, true, Some(json!({"nope": true})), None),
    )
    .await;
    assert_eq!(status, 400, "a malformed ack must be refused");
    let (status, _) = http_post(
        &base,
        RESULT,
        Some(&token),
        result_body(&pulled, false, None, None),
    )
    .await;
    assert_eq!(status, 400, "an undiagnosed failure must be refused");

    // Every rejection left the row claimed and completable.
    assert_eq!(job_state(&db.pool, job_id).await, "claimed");
    let (status, _) = http_post(
        &base,
        RESULT,
        Some(&token),
        result_body(&pulled, true, Some(ensure_ack(env, 3, "running")), None),
    )
    .await;
    assert_eq!(status, 204);
    assert_eq!(job_state(&db.pool, job_id).await, "succeeded");

    handle.abort();
    db.teardown().await;
}

#[tokio::test]
async fn unconfirmed_and_failed_outcomes_become_uncertain_blockers() {
    let db = DispatchDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "dispatch-uncertain-app").await;
    let (manager, token) = create_manager_with_token(&db.pool, "dispatch-uncertain-mgr").await;

    let stop_env = create_environment(&db.pool, app, "uncertain-stop").await;
    let stop_id = seed_job(
        &db.pool,
        app,
        stop_env,
        manager,
        "stop",
        0,
        "uncertain-stop-token",
        future_deadline(),
    )
    .await;
    let ensure_env = create_environment(&db.pool, app, "uncertain-ensure").await;
    let ensure_id = seed_job(
        &db.pool,
        app,
        ensure_env,
        manager,
        "ensure",
        0,
        "uncertain-ensure-token",
        future_deadline(),
    )
    .await;

    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;

    // A valid but unconfirmed stop is an uncertain blocker, not a failure.
    let pulled = pull(&base, &token).await;
    assert_eq!(pulled, format!("envjob:{stop_id}"));
    let (status, _) = http_post(
        &base,
        RESULT,
        Some(&token),
        result_body(
            &pulled,
            true,
            Some(stop_ack(stop_env, 0, false, "running")),
            None,
        ),
    )
    .await;
    assert_eq!(status, 204);
    assert_eq!(job_state(&db.pool, stop_id).await, "uncertain");
    let stored = job_result(&db.pool, stop_id).await.expect("stop result");
    assert_eq!(stored["outcome"], "stop_unconfirmed");
    assert_eq!(stored["confirmed"], false);

    // A failed operation is uncertain with a redacted fixed diagnostic and
    // never echoes the manager's raw error.
    let pulled = pull(&base, &token).await;
    assert_eq!(pulled, format!("envjob:{ensure_id}"));
    let (status, _) = http_post(
        &base,
        RESULT,
        Some(&token),
        result_body(&pulled, false, None, Some("secret=leak docker exploded")),
    )
    .await;
    assert_eq!(status, 204);
    assert_eq!(job_state(&db.pool, ensure_id).await, "uncertain");
    let stored = job_result(&db.pool, ensure_id)
        .await
        .expect("ensure result");
    assert_eq!(stored["outcome"], "ensure_uncertain");
    let summary = job_error_summary(&db.pool, ensure_id)
        .await
        .expect("redacted diagnostic");
    assert!(
        summary.contains("failed execution environment operation"),
        "fixed diagnostic expected, got {summary}"
    );
    assert!(
        !summary.contains("secret=leak"),
        "the manager error text must never be persisted"
    );

    // A late success cannot clear the uncertain blocker.
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
    assert_eq!(status, 409, "a late success must not clear uncertain");
    assert_eq!(job_state(&db.pool, ensure_id).await, "uncertain");

    handle.abort();
    db.teardown().await;
}

#[tokio::test]
async fn completion_after_the_deadline_never_mutates_the_row() {
    let db = DispatchDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "dispatch-deadline-app").await;
    let (manager, token) = create_manager_with_token(&db.pool, "dispatch-deadline-mgr").await;
    let env = create_environment(&db.pool, app, "deadline-ensure").await;
    let job_id = seed_job(
        &db.pool,
        app,
        env,
        manager,
        "ensure",
        0,
        "deadline-token",
        future_deadline(),
    )
    .await;

    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;
    let pulled = pull(&base, &token).await;
    assert_eq!(pulled, format!("envjob:{job_id}"));

    // Expire the claim exactly as elapsed wall-clock time would.
    sqlx::query(
        "UPDATE execution_environment_jobs \
         SET deadline_at = NOW() - INTERVAL '1 second' WHERE job_id = $1",
    )
    .bind(job_id)
    .execute(&db.pool)
    .await
    .expect("expire deadline");

    let (status, _) = http_post(
        &base,
        RESULT,
        Some(&token),
        result_body(&pulled, true, Some(ensure_ack(env, 0, "running")), None),
    )
    .await;
    assert_eq!(
        status, 409,
        "a completion past the deadline must be refused"
    );
    assert_eq!(
        job_state(&db.pool, job_id).await,
        "claimed",
        "an expired transport timeout stays unreclaimed, not silently cleared"
    );

    handle.abort();
    db.teardown().await;
}
