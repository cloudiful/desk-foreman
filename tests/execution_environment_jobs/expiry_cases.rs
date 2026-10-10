//! Expiry, deadline and reconnect cases for durable environment jobs.

use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use desk_foreman::db::queries::{
    ClaimOutcome, CompleteOutcome, EnqueueOutcome, claim_execution_environment_job,
    complete_execution_environment_job, enqueue_execution_environment_job,
    expire_execution_environment_jobs, find_active_execution_environment_job,
    find_execution_environment_job_by_token,
};

use super::fixtures::{
    create_environment, create_manager, future_deadline, past_deadline, seed_ensure,
};
use super::helpers::create_app_with_token;
use super::support::JobDb;

#[tokio::test]
async fn completion_after_deadline_never_succeeds_and_late_results_keep_the_blocker() {
    let db = JobDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "job-deadline").await;
    let manager = create_manager(&db.pool, "job-mgr-deadline", true).await;
    let env = create_environment(&db.pool, app, "deadline-user").await;
    let deadline = Utc::now() + ChronoDuration::milliseconds(300);
    let job_id = seed_ensure(&db.pool, app, env, manager, "deadline-token", deadline).await;

    match claim_execution_environment_job(&db.pool, app, env, "deadline-token", manager)
        .await
        .expect("claim")
    {
        ClaimOutcome::Claimed(_) => {}
        other => panic!("expected Claimed before deadline, got {other:?}"),
    }

    tokio::time::sleep(Duration::from_millis(500)).await;

    let late = complete_execution_environment_job(
        &db.pool,
        app,
        env,
        "deadline-token",
        job_id,
        manager,
        "succeeded",
        None,
        None,
    )
    .await
    .expect("late complete");
    assert!(
        !matches!(late, CompleteOutcome::Completed(_)),
        "completion after the deadline must never succeed, got {late:?}"
    );
    let current = find_execution_environment_job_by_token(&db.pool, app, env, "deadline-token")
        .await
        .expect("lookup")
        .expect("row");
    assert_eq!(current.state, "claimed");

    let expired = expire_execution_environment_jobs(&db.pool, app, env)
        .await
        .expect("expire");
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].state, "uncertain");

    let stale = complete_execution_environment_job(
        &db.pool,
        app,
        env,
        "deadline-token",
        job_id,
        manager,
        "succeeded",
        None,
        None,
    )
    .await
    .expect("stale complete");
    assert!(
        !matches!(stale, CompleteOutcome::Completed(_)),
        "a stale result must not clear the uncertain blocker, got {stale:?}"
    );

    let active = find_active_execution_environment_job(&db.pool, app, env)
        .await
        .expect("active lookup")
        .expect("uncertain blocker");
    assert_eq!(active.state, "uncertain");

    let blocked = enqueue_execution_environment_job(
        &db.pool,
        app,
        env,
        manager,
        "ensure",
        7,
        "post-expiry-token",
        future_deadline(),
    )
    .await
    .expect("post-expiry enqueue");
    assert!(
        matches!(blocked, EnqueueOutcome::ActiveConflict),
        "an uncertain record blocks a new ensure, got {blocked:?}"
    );

    db.teardown().await;
}

#[tokio::test]
async fn pending_expiry_becomes_uncertain_and_is_never_claimable() {
    let db = JobDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "job-pending-expiry").await;
    let manager = create_manager(&db.pool, "job-mgr-pending-expiry", true).await;
    let env = create_environment(&db.pool, app, "pending-expiry-user").await;
    seed_ensure(
        &db.pool,
        app,
        env,
        manager,
        "pending-expiry-token",
        past_deadline(),
    )
    .await;

    let expired = expire_execution_environment_jobs(&db.pool, app, env)
        .await
        .expect("expire");
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].state, "uncertain");
    assert!(expired[0].claimed_at.is_none());

    match claim_execution_environment_job(&db.pool, app, env, "pending-expiry-token", manager)
        .await
        .expect("claim expired")
    {
        ClaimOutcome::NotClaimable { current } => {
            assert_eq!(current.expect("own row").state, "uncertain");
        }
        other => panic!("expired pending job must not be claimable, got {other:?}"),
    }

    db.teardown().await;
}

#[tokio::test]
async fn uncertain_survives_reconnect_and_blocks_new_ensure_of_any_generation() {
    let db = JobDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "job-reconnect").await;
    let manager = create_manager(&db.pool, "job-mgr-reconnect", true).await;
    let env = create_environment(&db.pool, app, "reconnect-user").await;
    seed_ensure(
        &db.pool,
        app,
        env,
        manager,
        "reconnect-token",
        past_deadline(),
    )
    .await;

    let expired = expire_execution_environment_jobs(&db.pool, app, env)
        .await
        .expect("expire");
    assert_eq!(expired[0].state, "uncertain");

    let fresh = db.reconnect().await;
    let active = find_active_execution_environment_job(&fresh, app, env)
        .await
        .expect("active lookup on reconnect")
        .expect("uncertain must survive a reconnect");
    assert_eq!(active.state, "uncertain");

    let blocked = enqueue_execution_environment_job(
        &fresh,
        app,
        env,
        manager,
        "ensure",
        42,
        "reconnect-blocked-token",
        future_deadline(),
    )
    .await
    .expect("blocked enqueue on reconnect");
    assert!(
        matches!(blocked, EnqueueOutcome::ActiveConflict),
        "uncertain must block a new ensure of any generation, got {blocked:?}"
    );

    fresh.close().await;
    db.teardown().await;
}
