//! Claim and completion cases for durable environment jobs.

use desk_foreman::db::queries::{
    ClaimOutcome, CompleteOutcome, ExecutionEnvironmentJobResult, claim_execution_environment_job,
    complete_execution_environment_job,
};

use super::fixtures::{create_environment, create_manager, future_deadline, seed_ensure};
use super::helpers::create_app_with_token;
use super::support::JobDb;

#[tokio::test]
async fn only_assigned_manager_can_claim_and_complete() {
    let db = JobDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "job-assigned").await;
    let assigned = create_manager(&db.pool, "job-mgr-assigned", true).await;
    let other = create_manager(&db.pool, "job-mgr-other", true).await;
    let env = create_environment(&db.pool, app, "assigned-user").await;
    let job_id = seed_ensure(
        &db.pool,
        app,
        env,
        assigned,
        "assigned-token",
        future_deadline(),
    )
    .await;

    match claim_execution_environment_job(&db.pool, app, env, "assigned-token", other)
        .await
        .expect("wrong manager claim")
    {
        ClaimOutcome::NotClaimable { current } => {
            assert_eq!(current.expect("own row").state, "pending");
        }
        other => panic!("wrong manager must not claim, got {other:?}"),
    }

    let claimed = claim_execution_environment_job(&db.pool, app, env, "assigned-token", assigned)
        .await
        .expect("assigned claim");
    assert!(
        matches!(claimed, ClaimOutcome::Claimed(_)),
        "got {claimed:?}"
    );

    match complete_execution_environment_job(
        &db.pool,
        app,
        env,
        "assigned-token",
        job_id,
        other,
        "succeeded",
        None,
        None,
    )
    .await
    .expect("wrong manager complete")
    {
        CompleteOutcome::NotCompletable { current } => {
            assert_eq!(current.expect("own row").state, "claimed");
        }
        other => panic!("wrong manager must not complete, got {other:?}"),
    }

    let completed = complete_execution_environment_job(
        &db.pool,
        app,
        env,
        "assigned-token",
        job_id,
        assigned,
        "succeeded",
        None,
        None,
    )
    .await
    .expect("assigned complete");
    match completed {
        CompleteOutcome::Completed(row) => {
            assert_eq!(row.state, "succeeded");
            assert!(row.terminal_at.is_some());
        }
        other => panic!("expected Completed, got {other:?}"),
    }

    db.teardown().await;
}

#[tokio::test]
async fn competing_claims_yield_one_winner() {
    let db = JobDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "job-claim-race").await;
    let manager = create_manager(&db.pool, "job-mgr-claim-race", true).await;
    let env = create_environment(&db.pool, app, "claim-race-user").await;
    seed_ensure(
        &db.pool,
        app,
        env,
        manager,
        "claim-race-token",
        future_deadline(),
    )
    .await;

    let first_pool = db.pool.clone();
    let second_pool = db.pool.clone();
    let (first, second) = tokio::join!(
        claim_execution_environment_job(&first_pool, app, env, "claim-race-token", manager),
        claim_execution_environment_job(&second_pool, app, env, "claim-race-token", manager),
    );
    let first = first.expect("first claim");
    let second = second.expect("second claim");
    let winners = [&first, &second]
        .iter()
        .filter(|outcome| matches!(outcome, ClaimOutcome::Claimed(_)))
        .count();
    assert_eq!(winners, 1, "got {first:?} / {second:?}");

    db.teardown().await;
}

#[tokio::test]
async fn terminal_duplicate_replays_and_conflicting_replay_is_refused() {
    let db = JobDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "job-terminal").await;
    let manager = create_manager(&db.pool, "job-mgr-terminal", true).await;
    let env = create_environment(&db.pool, app, "terminal-user").await;
    let job_id = seed_ensure(
        &db.pool,
        app,
        env,
        manager,
        "terminal-token",
        future_deadline(),
    )
    .await;

    claim_execution_environment_job(&db.pool, app, env, "terminal-token", manager)
        .await
        .expect("claim");
    let result = ExecutionEnvironmentJobResult::stop_confirmed("exited");
    complete_execution_environment_job(
        &db.pool,
        app,
        env,
        "terminal-token",
        job_id,
        manager,
        "succeeded",
        Some(result.clone()),
        None,
    )
    .await
    .expect("first complete");

    let duplicate = complete_execution_environment_job(
        &db.pool,
        app,
        env,
        "terminal-token",
        job_id,
        manager,
        "succeeded",
        Some(result),
        None,
    )
    .await
    .expect("duplicate complete");
    assert!(
        matches!(duplicate, CompleteOutcome::AlreadyTerminal(_)),
        "identical replay must return the recorded outcome, got {duplicate:?}"
    );

    let conflict = complete_execution_environment_job(
        &db.pool,
        app,
        env,
        "terminal-token",
        job_id,
        manager,
        "failed",
        None,
        Some("late failure"),
    )
    .await
    .expect("conflicting complete");
    assert!(
        matches!(conflict, CompleteOutcome::TerminalConflict { .. }),
        "conflicting replay must be refused, got {conflict:?}"
    );

    db.teardown().await;
}
