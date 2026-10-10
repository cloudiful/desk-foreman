//! Reviewer-added verification for database-level ownership fencing, token
//! isolation across environments, and late-result handling after expiry.
//!
//! These cases are independent of the executor suite: they probe the composite
//! foreign key directly instead of only through the store's owner-scoped
//! `INSERT ... SELECT`, and they cover the unique-violation branch while the
//! caller already owns an active row.

use chrono::{Duration as ChronoDuration, Utc};
use desk_foreman::db::queries::{
    ClaimOutcome, CompleteOutcome, EnqueueOutcome, claim_execution_environment_job,
    complete_execution_environment_job, enqueue_execution_environment_job,
    expire_execution_environment_jobs, find_execution_environment_job_by_token,
};
use sqlx::PgPool;
use uuid::Uuid;

use super::fixtures::{create_environment, create_manager, future_deadline, past_deadline};
use super::helpers::create_app_with_token;
use super::support::JobDb;

/// The composite owner foreign key must reject a mismatched pair even when the
/// store is bypassed entirely.
#[tokio::test]
async fn database_rejects_a_mismatched_owner_pair_without_the_store() {
    let db = JobDb::setup().await;
    let (app_a, _) = create_app_with_token(&db.pool, "rev-fk-a").await;
    let (app_b, _) = create_app_with_token(&db.pool, "rev-fk-b").await;
    let manager = create_manager(&db.pool, "rev-fk-mgr", true).await;
    let env_a = create_environment(&db.pool, app_a, "fk-user").await;

    let direct = sqlx::query(
        "INSERT INTO execution_environment_jobs \
           (job_id, token, environment_id, application_id, assigned_manager_id, \
            job_kind, target_generation, deadline_at) \
         VALUES ($1, $2, $3, $4, $5, 'ensure', 0, NOW() + INTERVAL '1 hour')",
    )
    .bind(Uuid::new_v4())
    .bind("rev-fk-token")
    .bind(env_a)
    .bind(app_b)
    .bind(manager)
    .execute(&db.pool)
    .await;

    let error = direct.expect_err("a foreign application pair must be rejected in SQL");
    let code = match &error {
        sqlx::Error::Database(db) => db.code().map(|c| c.into_owned()),
        other => panic!("expected a database error, got {other}"),
    };
    assert_eq!(code.as_deref(), Some("23503"), "got {error}");

    db.teardown().await;
}

/// A token already held by another tenant stays invisible to a caller that
/// already owns an active row, i.e. the unique-violation branch leaks nothing.
#[tokio::test]
async fn token_of_another_tenant_is_never_returned_even_with_a_local_active_row() {
    let db = JobDb::setup().await;
    let (app_a, _) = create_app_with_token(&db.pool, "rev-leak-a").await;
    let (app_b, _) = create_app_with_token(&db.pool, "rev-leak-b").await;
    let manager = create_manager(&db.pool, "rev-leak-mgr", true).await;
    let env_a = create_environment(&db.pool, app_a, "leak-user").await;
    let env_b = create_environment(&db.pool, app_b, "leak-user").await;

    let first = enqueue_execution_environment_job(
        &db.pool,
        app_a,
        env_a,
        manager,
        "ensure",
        0,
        "rev-shared-token",
        future_deadline(),
    )
    .await
    .expect("tenant A enqueue");
    assert!(matches!(first, EnqueueOutcome::Created(_)));

    // Tenant B already owns an active stop row of its own.
    let local = enqueue_execution_environment_job(
        &db.pool,
        app_b,
        env_b,
        manager,
        "stop",
        0,
        "rev-local-token",
        future_deadline(),
    )
    .await
    .expect("tenant B local stop");
    assert!(matches!(local, EnqueueOutcome::Created(_)));

    let stolen = enqueue_execution_environment_job(
        &db.pool,
        app_b,
        env_b,
        manager,
        "stop",
        1,
        "rev-shared-token",
        future_deadline(),
    )
    .await
    .expect("tenant B foreign token");

    match stolen {
        // Both refusals are acceptable; returning tenant A's row is not.
        EnqueueOutcome::ActiveConflict | EnqueueOutcome::EnvironmentNotFound => {}
        other => panic!("a foreign token must never resolve to a row, got {other:?}"),
    }
    let visible =
        find_execution_environment_job_by_token(&db.pool, app_b, env_b, "rev-shared-token")
            .await
            .expect("lookup B");
    assert!(
        visible.is_none(),
        "tenant B must not read tenant A's row, got {visible:?}"
    );

    db.teardown().await;
}

/// One token is bound to one environment of one application.
#[tokio::test]
async fn same_application_token_on_another_environment_does_not_resolve() {
    let db = JobDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "rev-same-app").await;
    let manager = create_manager(&db.pool, "rev-same-app-mgr", true).await;
    let env_one = create_environment(&db.pool, app, "same-app-user-1").await;
    let env_two = create_environment(&db.pool, app, "same-app-user-2").await;

    let first = enqueue_execution_environment_job(
        &db.pool,
        app,
        env_one,
        manager,
        "ensure",
        0,
        "rev-one-token",
        future_deadline(),
    )
    .await
    .expect("first environment enqueue");
    assert!(matches!(first, EnqueueOutcome::Created(_)));

    let second = enqueue_execution_environment_job(
        &db.pool,
        app,
        env_two,
        manager,
        "ensure",
        0,
        "rev-one-token",
        future_deadline(),
    )
    .await
    .expect("second environment enqueue");
    assert!(
        matches!(second, EnqueueOutcome::EnvironmentNotFound),
        "a token must not resolve across environments of one application, got {second:?}"
    );

    let original = find_execution_environment_job_by_token(&db.pool, app, env_one, "rev-one-token")
        .await
        .expect("lookup original")
        .expect("original row");
    assert_eq!(original.environment_id, env_one);

    db.teardown().await;
}

/// A late result that matches the recorded `uncertain` outcome is idempotent,
/// and a contradicting late result is refused without mutating the blocker.
#[tokio::test]
async fn late_results_after_expiry_never_mutate_the_uncertain_record() {
    let db = JobDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "rev-late").await;
    let manager = create_manager(&db.pool, "rev-late-mgr", true).await;
    let env = create_environment(&db.pool, app, "late-user").await;

    let deadline = Utc::now() + ChronoDuration::milliseconds(300);
    let job_id = match enqueue_execution_environment_job(
        &db.pool,
        app,
        env,
        manager,
        "ensure",
        3,
        "rev-late-token",
        deadline,
    )
    .await
    .expect("enqueue")
    {
        EnqueueOutcome::Created(row) => row.job_id,
        other => panic!("expected Created, got {other:?}"),
    };
    assert!(
        matches!(
            claim_execution_environment_job(&db.pool, app, env, "rev-late-token", manager)
                .await
                .expect("claim"),
            ClaimOutcome::Claimed(_)
        ),
        "the assigned manager must claim before the deadline"
    );

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let expired = expire_execution_environment_jobs(&db.pool, app, env)
        .await
        .expect("expire");
    assert_eq!(expired.len(), 1);
    let recorded_at = expired[0].terminal_at.expect("uncertain terminal_at");
    assert_eq!(expired[0].target_generation, 3);

    // A contradicting late success is refused.
    match complete_execution_environment_job(
        &db.pool,
        app,
        env,
        "rev-late-token",
        job_id,
        manager,
        "succeeded",
        None,
        None,
    )
    .await
    .expect("late success")
    {
        CompleteOutcome::TerminalConflict { current } => {
            assert_eq!(current.state, "uncertain");
            assert_eq!(current.terminal_at, Some(recorded_at));
        }
        other => panic!("a late success must not overwrite uncertain, got {other:?}"),
    }

    // A matching replay of the recorded terminal state is idempotent.
    match complete_execution_environment_job(
        &db.pool,
        app,
        env,
        "rev-late-token",
        job_id,
        manager,
        "uncertain",
        None,
        None,
    )
    .await
    .expect("uncertain replay")
    {
        CompleteOutcome::AlreadyTerminal(current) => {
            assert_eq!(current.state, "uncertain");
            assert_eq!(current.terminal_at, Some(recorded_at));
        }
        other => panic!("an identical terminal replay must be idempotent, got {other:?}"),
    }

    // Expiry is idempotent and the blocker keeps forbidding new work.
    let again = expire_execution_environment_jobs(&db.pool, app, env)
        .await
        .expect("second expire");
    assert!(again.is_empty(), "expiry must not re-terminal a row");
    let active = expire_execution_environment_jobs(&db.pool, app, Uuid::new_v4())
        .await
        .expect("foreign environment expiry");
    assert!(active.is_empty(), "expiry is owner-scoped");

    let blocked = enqueue_execution_environment_job(
        &db.pool,
        app,
        env,
        manager,
        "ensure",
        99,
        "rev-late-blocked",
        future_deadline(),
    )
    .await
    .expect("blocked enqueue");
    assert!(
        matches!(blocked, EnqueueOutcome::ActiveConflict),
        "the uncertain record must still block, got {blocked:?}"
    );

    db.teardown().await;
}

/// Expiry of one tenant never touches another tenant's expired rows.
#[tokio::test]
async fn expiry_only_touches_the_scoped_owner_environment() {
    let db = JobDb::setup().await;
    let (app_a, _) = create_app_with_token(&db.pool, "rev-exp-a").await;
    let (app_b, _) = create_app_with_token(&db.pool, "rev-exp-b").await;
    let manager = create_manager(&db.pool, "rev-exp-mgr", true).await;
    let env_a = create_environment(&db.pool, app_a, "exp-user").await;
    let env_b = create_environment(&db.pool, app_b, "exp-user").await;

    for (app, env, token) in [
        (app_a, env_a, "rev-exp-token-a"),
        (app_b, env_b, "rev-exp-token-b"),
    ] {
        assert!(
            matches!(
                enqueue_execution_environment_job(
                    &db.pool,
                    app,
                    env,
                    manager,
                    "ensure",
                    0,
                    token,
                    past_deadline(),
                )
                .await
                .expect("enqueue"),
                EnqueueOutcome::Created(_)
            ),
            "each tenant must record its own expired intent"
        );
    }

    let expired = expire_execution_environment_jobs(&db.pool, app_a, env_a)
        .await
        .expect("expire A");
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].application_id, app_a);

    let untouched: String =
        sqlx::query_scalar("SELECT state FROM execution_environment_jobs WHERE token = $1")
            .bind("rev-exp-token-b")
            .fetch_one(&db.pool)
            .await
            .expect("read B row");
    assert_eq!(
        untouched, "pending",
        "another tenant's expired row must not be transitioned"
    );

    let pool: &PgPool = &db.pool;
    assert!(!pool.is_closed());

    db.teardown().await;
}

/// A pending job whose deadline has passed is refused by the claim statement
/// itself, before any expiry sweep runs: the guard is the deadline predicate in
/// the compare-and-set, not the sweep.
#[tokio::test]
async fn claim_refuses_an_expired_pending_job_before_any_expiry_sweep() {
    let db = JobDb::setup().await;
    let (app, _) = create_app_with_token(&db.pool, "rev-claim-deadline").await;
    let manager = create_manager(&db.pool, "rev-claim-deadline-mgr", true).await;
    let env = create_environment(&db.pool, app, "claim-deadline-user").await;

    assert!(
        matches!(
            enqueue_execution_environment_job(
                &db.pool,
                app,
                env,
                manager,
                "ensure",
                0,
                "rev-claim-deadline-token",
                past_deadline(),
            )
            .await
            .expect("enqueue"),
            EnqueueOutcome::Created(_)
        ),
        "an intent with a past deadline is still recorded, it is only unclaimable"
    );

    match claim_execution_environment_job(&db.pool, app, env, "rev-claim-deadline-token", manager)
        .await
        .expect("claim after deadline")
    {
        ClaimOutcome::NotClaimable { current } => {
            let row = current.expect("own row");
            assert_eq!(row.state, "pending");
            assert!(
                row.claimed_at.is_none(),
                "a refused claim must not mark the row claimed"
            );
        }
        other => panic!("an expired pending job must never be claimed, got {other:?}"),
    }

    // The row stays an active blocker until the fail-closed sweep records it.
    let blocked = enqueue_execution_environment_job(
        &db.pool,
        app,
        env,
        manager,
        "ensure",
        5,
        "rev-claim-deadline-blocked",
        future_deadline(),
    )
    .await
    .expect("blocked enqueue");
    assert!(
        matches!(blocked, EnqueueOutcome::ActiveConflict),
        "an unexpired new ensure must still be refused while the old one is active, got {blocked:?}"
    );

    db.teardown().await;
}
