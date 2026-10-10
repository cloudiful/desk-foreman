//! Enqueue, replay and tenancy fencing cases.

use desk_foreman::db::queries::{
    EnqueueOutcome, enqueue_execution_environment_job, find_execution_environment_job_by_token,
};

use super::fixtures::{count_jobs, create_environment, create_manager, future_deadline};
use super::helpers::create_app_with_token;
use super::support::JobDb;

#[tokio::test]
async fn foreign_application_pair_is_rejected_without_recording_a_row() {
    let db = JobDb::setup().await;
    let (app_a, _) = create_app_with_token(&db.pool, "job-tenant-a").await;
    let (app_b, _) = create_app_with_token(&db.pool, "job-tenant-b").await;
    let manager = create_manager(&db.pool, "job-mgr-tenant", true).await;
    let env = create_environment(&db.pool, app_a, "user-1").await;

    let foreign = enqueue_execution_environment_job(
        &db.pool,
        app_b,
        env,
        manager,
        "ensure",
        0,
        "tenant-token",
        future_deadline(),
    )
    .await
    .expect("foreign enqueue");
    assert!(
        matches!(foreign, EnqueueOutcome::EnvironmentNotFound),
        "foreign owner pair must not be accepted, got {foreign:?}"
    );
    assert_eq!(count_jobs(&db.pool, env).await, 0);

    let owned = enqueue_execution_environment_job(
        &db.pool,
        app_a,
        env,
        manager,
        "ensure",
        0,
        "tenant-token",
        future_deadline(),
    )
    .await
    .expect("owned enqueue");
    assert!(
        matches!(owned, EnqueueOutcome::Created(_)),
        "owner pair must be accepted, got {owned:?}"
    );

    db.teardown().await;
}

#[tokio::test]
async fn identical_token_replay_returns_the_original_row() {
    let db = JobDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "job-replay").await;
    let manager = create_manager(&db.pool, "job-mgr-replay", true).await;
    let env = create_environment(&db.pool, app, "replay-user").await;

    let first = enqueue_execution_environment_job(
        &db.pool,
        app,
        env,
        manager,
        "ensure",
        2,
        "replay-token",
        future_deadline(),
    )
    .await
    .expect("first enqueue");
    let first_id = match first {
        EnqueueOutcome::Created(row) => row.job_id,
        other => panic!("expected Created, got {other:?}"),
    };

    let replay = enqueue_execution_environment_job(
        &db.pool,
        app,
        env,
        manager,
        "ensure",
        2,
        "replay-token",
        future_deadline(),
    )
    .await
    .expect("replay enqueue");
    match replay {
        EnqueueOutcome::Replayed(row) => assert_eq!(row.job_id, first_id),
        other => panic!("expected Replayed, got {other:?}"),
    }
    assert_eq!(count_jobs(&db.pool, env).await, 1);

    db.teardown().await;
}

#[tokio::test]
async fn conflicting_token_intent_is_refused() {
    let db = JobDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "job-conflict").await;
    let manager = create_manager(&db.pool, "job-mgr-conflict", true).await;
    let env = create_environment(&db.pool, app, "conflict-user").await;

    let created = enqueue_execution_environment_job(
        &db.pool,
        app,
        env,
        manager,
        "ensure",
        0,
        "conflict-token",
        future_deadline(),
    )
    .await
    .expect("first enqueue");
    let created_id = match created {
        EnqueueOutcome::Created(row) => row.job_id,
        other => panic!("expected Created, got {other:?}"),
    };

    let conflicting = enqueue_execution_environment_job(
        &db.pool,
        app,
        env,
        manager,
        "ensure",
        1,
        "conflict-token",
        future_deadline(),
    )
    .await
    .expect("conflicting enqueue");
    match conflicting {
        EnqueueOutcome::TokenConflict { current } => {
            assert_eq!(current.job_id, created_id);
            assert_eq!(current.target_generation, 0);
        }
        other => panic!("expected TokenConflict, got {other:?}"),
    }
    assert_eq!(count_jobs(&db.pool, env).await, 1);

    db.teardown().await;
}

#[tokio::test]
async fn cross_tenant_token_never_leaks_another_row() {
    let db = JobDb::setup().await;
    let (app_a, _) = create_app_with_token(&db.pool, "job-leak-a").await;
    let (app_b, _) = create_app_with_token(&db.pool, "job-leak-b").await;
    let manager = create_manager(&db.pool, "job-mgr-leak", true).await;
    let env_a = create_environment(&db.pool, app_a, "leak-user").await;
    let env_b = create_environment(&db.pool, app_b, "leak-user").await;

    let created = enqueue_execution_environment_job(
        &db.pool,
        app_a,
        env_a,
        manager,
        "ensure",
        0,
        "shared-token",
        future_deadline(),
    )
    .await
    .expect("tenant A enqueue");
    assert!(matches!(created, EnqueueOutcome::Created(_)));

    let stolen = enqueue_execution_environment_job(
        &db.pool,
        app_b,
        env_b,
        manager,
        "ensure",
        0,
        "shared-token",
        future_deadline(),
    )
    .await
    .expect("tenant B enqueue");
    assert!(
        matches!(stolen, EnqueueOutcome::EnvironmentNotFound),
        "tenant B must not receive tenant A's row, got {stolen:?}"
    );
    assert_eq!(count_jobs(&db.pool, env_b).await, 0);

    let still_there =
        find_execution_environment_job_by_token(&db.pool, app_a, env_a, "shared-token")
            .await
            .expect("lookup A")
            .expect("tenant A row must remain");
    assert_eq!(still_there.application_id, app_a);

    db.teardown().await;
}
