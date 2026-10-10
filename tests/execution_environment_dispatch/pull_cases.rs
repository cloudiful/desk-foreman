//! Pull (claim) cases for durable assigned-manager jobs (issue 742 P2).

use desk_foreman::db::queries::{
    CompleteOutcome, claim_execution_environment_job, complete_execution_environment_job,
};

use super::helpers::{create_app_with_token, test_state};
use super::http::{http_get, spawn_http};
use super::support::{
    DispatchDb, create_environment, create_manager_with_token, future_deadline, job_state,
    past_deadline, seed_job,
};

const NEXT: &str = "/api/internal/runner-manager/jobs/next";

#[tokio::test]
async fn assigned_manager_pulls_and_other_manager_cannot() {
    let db = DispatchDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "dispatch-pull-app").await;
    let (manager_a, token_a) = create_manager_with_token(&db.pool, "dispatch-pull-a").await;
    let (_, token_b) = create_manager_with_token(&db.pool, "dispatch-pull-b").await;
    let env = create_environment(&db.pool, app, "pull-user").await;
    let job_id = seed_job(
        &db.pool,
        app,
        env,
        manager_a,
        "ensure",
        2,
        "pull-token",
        future_deadline(),
    )
    .await;

    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;

    let (status, body) = http_get(&base, NEXT, Some(&token_a)).await;
    assert_eq!(status, 200, "assigned manager pull must succeed");
    assert_eq!(body["job_id"], format!("envjob:{job_id}"));
    assert_eq!(body["kind"], "ensure_execution_environment");
    assert_eq!(
        body["payload"]["owner"]["ExecutionEnvironment"]["environment_id"],
        env.to_string()
    );
    assert_eq!(
        body["payload"]["owner"]["ExecutionEnvironment"]["generation"],
        2
    );
    assert_eq!(job_state(&db.pool, job_id).await, "claimed");

    // The other manager sees nothing: the row is claimed and it has no
    // claimable durable job of its own. This still exercises the legacy
    // fall-through, which returns null.
    let (status_b, body_b) = http_get(&base, NEXT, Some(&token_b)).await;
    assert_eq!(status_b, 200);
    assert!(
        body_b.is_null(),
        "a foreign manager must not pull the job, got {body_b}"
    );

    handle.abort();
    db.teardown().await;
}

#[tokio::test]
async fn concurrent_polls_claim_the_job_once() {
    let db = DispatchDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "dispatch-race-app").await;
    let (manager, token) = create_manager_with_token(&db.pool, "dispatch-race-mgr").await;
    let env = create_environment(&db.pool, app, "race-user").await;
    let job_id = seed_job(
        &db.pool,
        app,
        env,
        manager,
        "ensure",
        0,
        "race-token",
        future_deadline(),
    )
    .await;

    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;

    let first_base = base.clone();
    let second_base = base.clone();
    let first_token = token.clone();
    let second_token = token.clone();
    let first = tokio::spawn(async move { http_get(&first_base, NEXT, Some(&first_token)).await });
    let second =
        tokio::spawn(async move { http_get(&second_base, NEXT, Some(&second_token)).await });
    let (_, first_body) = first.await.expect("first poll");
    let (_, second_body) = second.await.expect("second poll");

    let claimed = [&first_body, &second_body]
        .iter()
        .filter(|body| !body.is_null())
        .count();
    assert_eq!(claimed, 1, "exactly one poll must claim the job");
    assert_eq!(job_state(&db.pool, job_id).await, "claimed");

    handle.abort();
    db.teardown().await;
}

#[tokio::test]
async fn expired_claimed_and_uncertain_rows_are_never_pulled() {
    let db = DispatchDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "dispatch-skip-app").await;
    let (manager, token) = create_manager_with_token(&db.pool, "dispatch-skip-mgr").await;

    // An expired pending row, a claimed row, and an uncertain row, each on
    // its own environment, plus one genuinely claimable pending row.
    let expired_env = create_environment(&db.pool, app, "skip-expired").await;
    let expired = seed_job(
        &db.pool,
        app,
        expired_env,
        manager,
        "ensure",
        0,
        "skip-expired-token",
        past_deadline(),
    )
    .await;

    let claimed_env = create_environment(&db.pool, app, "skip-claimed").await;
    let claimed = seed_job(
        &db.pool,
        app,
        claimed_env,
        manager,
        "ensure",
        0,
        "skip-claimed-token",
        future_deadline(),
    )
    .await;
    claim_execution_environment_job(&db.pool, app, claimed_env, "skip-claimed-token", manager)
        .await
        .expect("claim");

    let uncertain_env = create_environment(&db.pool, app, "skip-uncertain").await;
    let uncertain = seed_job(
        &db.pool,
        app,
        uncertain_env,
        manager,
        "ensure",
        0,
        "skip-uncertain-token",
        future_deadline(),
    )
    .await;
    claim_execution_environment_job(
        &db.pool,
        app,
        uncertain_env,
        "skip-uncertain-token",
        manager,
    )
    .await
    .expect("claim uncertain");
    match complete_execution_environment_job(
        &db.pool,
        app,
        uncertain_env,
        "skip-uncertain-token",
        uncertain,
        manager,
        "uncertain",
        None,
        Some("transport stalled"),
    )
    .await
    .expect("complete uncertain")
    {
        CompleteOutcome::Completed(_) => {}
        other => panic!("expected uncertain completion, got {other:?}"),
    }

    let ready_env = create_environment(&db.pool, app, "skip-ready").await;
    let ready = seed_job(
        &db.pool,
        app,
        ready_env,
        manager,
        "ensure",
        0,
        "skip-ready-token",
        future_deadline(),
    )
    .await;

    let (base, handle) = spawn_http(test_state(db.pool.clone())).await;

    let (status, body) = http_get(&base, NEXT, Some(&token)).await;
    assert_eq!(status, 200);
    assert_eq!(
        body["job_id"],
        format!("envjob:{ready}"),
        "the claimable row must be pulled while non-claimable rows are skipped"
    );

    // Non-claimable rows keep their state untouched.
    assert_eq!(job_state(&db.pool, expired).await, "pending");
    assert_eq!(job_state(&db.pool, claimed).await, "claimed");
    assert_eq!(job_state(&db.pool, uncertain).await, "uncertain");

    // With nothing claimable left the pull falls through and returns null.
    let (status, body) = http_get(&base, NEXT, Some(&token)).await;
    assert_eq!(status, 200);
    assert!(
        body.is_null(),
        "no claimable row must return null, got {body}"
    );

    handle.abort();
    db.teardown().await;
}
