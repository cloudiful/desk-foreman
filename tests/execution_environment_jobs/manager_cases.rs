//! Manager validation and active-operation concurrency cases.

use desk_foreman::db::queries::{EnqueueOutcome, enqueue_execution_environment_job};

use super::fixtures::{count_jobs, create_environment, create_manager, future_deadline};
use super::helpers::create_app_with_token;
use super::support::JobDb;

#[tokio::test]
async fn missing_or_disabled_manager_is_rejected() {
    let db = JobDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "job-manager").await;
    let disabled = create_manager(&db.pool, "job-mgr-disabled", false).await;
    let env = create_environment(&db.pool, app, "manager-user").await;

    let disabled_outcome = enqueue_execution_environment_job(
        &db.pool,
        app,
        env,
        disabled,
        "ensure",
        0,
        "disabled-token",
        future_deadline(),
    )
    .await
    .expect("disabled manager enqueue");
    assert!(matches!(
        disabled_outcome,
        EnqueueOutcome::EnvironmentNotFound
    ));

    let missing_outcome = enqueue_execution_environment_job(
        &db.pool,
        app,
        env,
        9_999_999,
        "ensure",
        0,
        "missing-token",
        future_deadline(),
    )
    .await
    .expect("missing manager enqueue");
    assert!(matches!(
        missing_outcome,
        EnqueueOutcome::EnvironmentNotFound
    ));
    assert_eq!(count_jobs(&db.pool, env).await, 0);

    db.teardown().await;
}

#[tokio::test]
async fn concurrent_ensure_across_generations_creates_one_active_row() {
    let db = JobDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "job-ensure-race").await;
    let manager = create_manager(&db.pool, "job-mgr-ensure-race", true).await;
    let env = create_environment(&db.pool, app, "ensure-race-user").await;

    let mut handles = Vec::new();
    for generation in 0..8_i64 {
        let pool = db.pool.clone();
        handles.push(tokio::spawn(async move {
            enqueue_execution_environment_job(
                &pool,
                app,
                env,
                manager,
                "ensure",
                generation,
                &format!("ensure-race-{generation}"),
                future_deadline(),
            )
            .await
        }));
    }

    let mut created = 0;
    let mut conflicts = 0;
    for handle in handles {
        match handle.await.expect("join").expect("enqueue") {
            EnqueueOutcome::Created(_) => created += 1,
            EnqueueOutcome::ActiveConflict => conflicts += 1,
            other => panic!("unexpected outcome {other:?}"),
        }
    }
    assert_eq!(created, 1, "exactly one active ensure may win");
    assert_eq!(conflicts, 7);

    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_environment_jobs \
         WHERE environment_id = $1 AND job_kind = 'ensure' \
           AND state IN ('pending', 'claimed', 'uncertain')",
    )
    .bind(env)
    .fetch_one(&db.pool)
    .await
    .expect("count active");
    assert_eq!(active, 1);

    db.teardown().await;
}

#[tokio::test]
async fn active_ensure_and_stop_records_coexist_but_each_kind_is_singular() {
    let db = JobDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "job-coexist").await;
    let manager = create_manager(&db.pool, "job-mgr-coexist", true).await;
    let env = create_environment(&db.pool, app, "coexist-user").await;

    let ensure = enqueue_execution_environment_job(
        &db.pool,
        app,
        env,
        manager,
        "ensure",
        0,
        "coexist-ensure",
        future_deadline(),
    )
    .await
    .expect("ensure");
    assert!(matches!(ensure, EnqueueOutcome::Created(_)));

    let stop = enqueue_execution_environment_job(
        &db.pool,
        app,
        env,
        manager,
        "stop",
        0,
        "coexist-stop",
        future_deadline(),
    )
    .await
    .expect("stop");
    assert!(
        matches!(stop, EnqueueOutcome::Created(_)),
        "an active stop is a separate record, got {stop:?}"
    );

    let second_stop = enqueue_execution_environment_job(
        &db.pool,
        app,
        env,
        manager,
        "stop",
        1,
        "coexist-stop-2",
        future_deadline(),
    )
    .await
    .expect("second stop");
    assert!(matches!(second_stop, EnqueueOutcome::ActiveConflict));

    assert_eq!(count_jobs(&db.pool, env).await, 2);

    db.teardown().await;
}
