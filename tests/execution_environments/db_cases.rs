//! Database fencing cases for durable execution environments.
//!
//! Proves idempotent ensure, concurrent convergence, tenant isolation,
//! atomic invalidate fencing and runtime-kind conflict preservation
//! against an isolated nonproduction PostgreSQL.

use desk_foreman::db::queries::{
    EnsureOutcome, InvalidateOutcome, ensure_execution_environment, find_execution_environment,
    invalidate_execution_environment,
};

use super::helpers::{IsolatedDb, create_app_with_token};

#[tokio::test]
async fn ensure_is_idempotent_for_repeated_calls() {
    let db = IsolatedDb::setup().await;
    let (app_id, _) = create_app_with_token(&db.pool, "ensure-idem").await;

    let first = ensure_execution_environment(&db.pool, app_id, "user-1", "opencode")
        .await
        .expect("first ensure");
    let (first_id, first_gen, first_ref, first_state) = match first {
        EnsureOutcome::Created(row) => {
            assert_eq!(row.generation, 0);
            assert_eq!(row.state, "starting");
            assert!(row.state_volume_ref.starts_with("df-env-"));
            (
                row.environment_id,
                row.generation,
                row.state_volume_ref,
                row.state,
            )
        }
        other => panic!("expected Created, got {other:?}"),
    };

    let second = ensure_execution_environment(&db.pool, app_id, "user-1", "opencode")
        .await
        .expect("second ensure");
    match second {
        EnsureOutcome::Existing(row) => {
            assert_eq!(row.environment_id, first_id);
            assert_eq!(row.generation, first_gen);
            assert_eq!(row.state_volume_ref, first_ref);
            assert_eq!(row.state, first_state);
        }
        other => panic!("expected Existing, got {other:?}"),
    }

    db.teardown().await;
}

#[tokio::test]
async fn concurrent_ensures_converge_on_one_identity() {
    let db = IsolatedDb::setup().await;
    let (app_id, _) = create_app_with_token(&db.pool, "ensure-race").await;

    let mut handles = Vec::new();
    for _ in 0..8 {
        let pool = db.pool.clone();
        handles.push(tokio::spawn(async move {
            ensure_execution_environment(&pool, app_id, "race-user", "opencode")
                .await
                .expect("concurrent ensure")
        }));
    }
    let mut ids = Vec::new();
    for handle in handles {
        match handle.await.expect("join") {
            EnsureOutcome::Created(row) | EnsureOutcome::Existing(row) => {
                ids.push(row.environment_id)
            }
            EnsureOutcome::RuntimeConflict { .. } => panic!("unexpected runtime conflict"),
        }
    }
    let first = ids[0];
    for id in &ids {
        assert_eq!(*id, first, "concurrent ensures must keep identity");
    }
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution_environments WHERE application_id = $1")
            .bind(app_id)
            .fetch_one(&db.pool)
            .await
            .expect("count rows");
    assert_eq!(count, 1);

    db.teardown().await;
}

#[tokio::test]
async fn two_applications_same_external_user_get_independent_rows() {
    let db = IsolatedDb::setup().await;
    let (app_a, _) = create_app_with_token(&db.pool, "tenant-a").await;
    let (app_b, _) = create_app_with_token(&db.pool, "tenant-b").await;

    let row_a = match ensure_execution_environment(&db.pool, app_a, "shared-user", "opencode")
        .await
        .expect("ensure A")
    {
        EnsureOutcome::Created(row) => row,
        other => panic!("expected Created for A, got {other:?}"),
    };
    let row_b = match ensure_execution_environment(&db.pool, app_b, "shared-user", "opencode")
        .await
        .expect("ensure B")
    {
        EnsureOutcome::Created(row) => row,
        other => panic!("expected Created for B, got {other:?}"),
    };

    assert_ne!(row_a.environment_id, row_b.environment_id);
    assert_eq!(row_a.application_id, app_a);
    assert_eq!(row_b.application_id, app_b);
    assert_ne!(row_a.state_volume_ref, row_b.state_volume_ref);

    db.teardown().await;
}

#[tokio::test]
async fn foreign_invalidate_returns_not_found_without_mutation() {
    let db = IsolatedDb::setup().await;
    let (app_a, _) = create_app_with_token(&db.pool, "foreign-a").await;
    let (app_b, _) = create_app_with_token(&db.pool, "foreign-b").await;

    let row = match ensure_execution_environment(&db.pool, app_a, "victim", "opencode")
        .await
        .expect("ensure")
    {
        EnsureOutcome::Created(row) => row,
        other => panic!("expected Created, got {other:?}"),
    };

    match invalidate_execution_environment(&db.pool, app_b, row.environment_id, 0)
        .await
        .expect("foreign invalidate")
    {
        InvalidateOutcome::NotFound => {}
        other => panic!("expected NotFound for foreign id, got {other:?}"),
    }

    let current = find_execution_environment(&db.pool, app_a, row.environment_id)
        .await
        .expect("lookup")
        .expect("row must remain");
    assert_eq!(current.generation, 0);
    assert_eq!(current.state, "starting");

    db.teardown().await;
}

#[tokio::test]
async fn competing_invalidations_yield_one_winner() {
    let db = IsolatedDb::setup().await;
    let (app_id, _) = create_app_with_token(&db.pool, "invalidate-race").await;

    let row = match ensure_execution_environment(&db.pool, app_id, "racer", "opencode")
        .await
        .expect("ensure")
    {
        EnsureOutcome::Created(row) => row,
        other => panic!("expected Created, got {other:?}"),
    };

    let pool_a = db.pool.clone();
    let pool_b = db.pool.clone();
    let id = row.environment_id;
    let (first, second) = tokio::join!(
        invalidate_execution_environment(&pool_a, app_id, id, 0),
        invalidate_execution_environment(&pool_b, app_id, id, 0),
    );
    let first = first.expect("first invalidate");
    let second = second.expect("second invalidate");

    let successes = [&first, &second]
        .iter()
        .filter(|o| matches!(o, InvalidateOutcome::Invalidated(_)))
        .count();
    let stales = [&first, &second]
        .iter()
        .filter(|o| matches!(o, InvalidateOutcome::Stale { .. }))
        .count();
    assert_eq!(
        successes, 1,
        "exactly one winner, got {first:?} / {second:?}"
    );
    assert_eq!(stales, 1);

    let current = find_execution_environment(&db.pool, app_id, id)
        .await
        .expect("lookup")
        .expect("row exists");
    assert_eq!(current.generation, 1);
    assert_eq!(current.state, "stopping");

    db.teardown().await;
}

#[tokio::test]
async fn stale_invalidate_leaves_generation_unchanged() {
    let db = IsolatedDb::setup().await;
    let (app_id, _) = create_app_with_token(&db.pool, "stale-check").await;

    let row = match ensure_execution_environment(&db.pool, app_id, "stale-user", "opencode")
        .await
        .expect("ensure")
    {
        EnsureOutcome::Created(row) => row,
        other => panic!("expected Created, got {other:?}"),
    };

    match invalidate_execution_environment(&db.pool, app_id, row.environment_id, 0)
        .await
        .expect("first invalidate")
    {
        InvalidateOutcome::Invalidated(updated) => {
            assert_eq!(updated.generation, 1);
            assert_eq!(updated.state, "stopping");
        }
        other => panic!("expected Invalidated, got {other:?}"),
    }

    match invalidate_execution_environment(&db.pool, app_id, row.environment_id, 0)
        .await
        .expect("stale invalidate")
    {
        InvalidateOutcome::Stale { current } => {
            assert_eq!(current.generation, 1);
            assert_eq!(current.state, "stopping");
        }
        other => panic!("expected Stale, got {other:?}"),
    }

    let current = find_execution_environment(&db.pool, app_id, row.environment_id)
        .await
        .expect("lookup")
        .expect("row exists");
    assert_eq!(current.generation, 1);
    assert_eq!(current.state, "stopping");

    db.teardown().await;
}

#[tokio::test]
async fn runtime_kind_conflict_preserves_row() {
    let db = IsolatedDb::setup().await;
    let (app_id, _) = create_app_with_token(&db.pool, "runtime-conflict").await;

    let created = match ensure_execution_environment(&db.pool, app_id, "conflict-user", "opencode")
        .await
        .expect("ensure")
    {
        EnsureOutcome::Created(row) => row,
        other => panic!("expected Created, got {other:?}"),
    };

    match ensure_execution_environment(&db.pool, app_id, "conflict-user", "other-kind")
        .await
        .expect("conflicting ensure")
    {
        EnsureOutcome::RuntimeConflict { current } => {
            assert_eq!(current.environment_id, created.environment_id);
            assert_eq!(current.runtime_kind, "opencode");
        }
        other => panic!("expected RuntimeConflict, got {other:?}"),
    }

    let current = find_execution_environment(&db.pool, app_id, created.environment_id)
        .await
        .expect("lookup")
        .expect("row exists");
    assert_eq!(current.runtime_kind, "opencode");
    assert_eq!(current.generation, 0);
    assert_eq!(current.state_volume_ref, created.state_volume_ref);

    db.teardown().await;
}
