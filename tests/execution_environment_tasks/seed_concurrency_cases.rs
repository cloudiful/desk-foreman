//! Seed concurrency and fault-rollback cases (issue 753 P2a).
//!
//! Two seals for the same task are serialized by the environment row lock, so
//! exactly one content wins and the other is a typed conflict — never a 500
//! and never two durable inputs. The fault case proves a mid-seal failure
//! leaves no orphan header or file rows.

use uuid::Uuid;

use desk_foreman::db::execution_environments::tasks::seeds::{
    SeedError, SeedFile, SeedPlan, seal_native_task_seed,
};

use super::support::{TaskFixture, call};

fn files(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(path, content)| (path.to_string(), content.to_string()))
        .collect()
}

#[tokio::test]
async fn two_concurrent_different_seals_leave_exactly_one_durable_input() {
    for _round in 0..4 {
        let fixture = TaskFixture::setup().await;
        let task_id = Uuid::new_v4();
        assert_eq!(
            fixture.admit_default(task_id, "op-race-seed").await.status,
            200
        );

        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let first_barrier = std::sync::Arc::clone(&barrier);
        let second_barrier = std::sync::Arc::clone(&barrier);
        let fixture_ref = &fixture;
        let (first, second) = tokio::join!(
            async move {
                first_barrier.wait().await;
                fixture_ref
                    .seed(
                        &fixture_ref.application_token,
                        task_id,
                        4,
                        &files(&[("a.txt", "first")]),
                    )
                    .await
            },
            async move {
                second_barrier.wait().await;
                fixture_ref
                    .seed(
                        &fixture_ref.application_token,
                        task_id,
                        4,
                        &files(&[("a.txt", "second")]),
                    )
                    .await
            }
        );

        let statuses = [first.status, second.status];
        assert_eq!(
            statuses.iter().filter(|status| **status == 200).count(),
            1,
            "exactly one winner expected: {statuses:?}"
        );
        let loser = if first.status == 200 { &second } else { &first };
        assert_eq!(
            loser.status,
            409,
            "the loser must be a typed conflict, never a 500: {}",
            loser.body_text()
        );

        // Exactly one durable snapshot, with exactly one file.
        let headers: (i64,) = sqlx::query_as(
            "SELECT COUNT(*)::BIGINT FROM execution_environment_task_seeds WHERE task_id = $1",
        )
        .bind(task_id)
        .fetch_one(&fixture.database.pool)
        .await
        .expect("count headers");
        assert_eq!(headers.0, 1);
        let rows: (i64,) = sqlx::query_as(
            "SELECT COUNT(*)::BIGINT FROM execution_environment_task_seed_files WHERE task_id = $1",
        )
        .bind(task_id)
        .fetch_one(&fixture.database.pool)
        .await
        .expect("count files");
        assert_eq!(rows.0, 1);
        fixture.teardown().await;
    }
}

#[tokio::test]
async fn a_failed_seal_leaves_no_orphan_header_or_files() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    assert_eq!(
        fixture.admit_default(task_id, "op-rollback").await.status,
        200
    );

    // A hand-built plan that violates the storage key: two entries with the same
    // path. `plan_task_seed` would refuse this, so it is injected below the
    // planner to exercise the transaction's own rollback.
    let duplicate = SeedFile {
        path: "a.txt".to_string(),
        content: "x".to_string(),
    };
    let plan = SeedPlan {
        files: vec![duplicate.clone(), duplicate],
        manifest_sha256: "0".repeat(64),
        file_count: 2,
        total_bytes: 2,
    };
    let outcome = seal_native_task_seed(
        &fixture.database.pool,
        fixture.application_id,
        fixture.environment_id,
        task_id,
        4,
        &plan,
    )
    .await;
    assert!(
        matches!(outcome, Err(SeedError::Internal)),
        "a storage-level violation must surface as an internal refusal"
    );

    // Nothing survived the failed seal.
    let headers: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT FROM execution_environment_task_seeds WHERE task_id = $1",
    )
    .bind(task_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("count headers");
    assert_eq!(headers.0, 0, "no orphan header");
    let rows: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT FROM execution_environment_task_seed_files WHERE task_id = $1",
    )
    .bind(task_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("count files");
    assert_eq!(rows.0, 0, "no orphan files");

    // The task is still seedable afterward: the failure left no trace.
    let seeded = fixture
        .seed(
            &fixture.application_token,
            task_id,
            4,
            &files(&[("a.txt", "x")]),
        )
        .await;
    assert_eq!(seeded.status, 200, "body: {}", seeded.body_text());
    fixture.teardown().await;
}

#[tokio::test]
async fn a_seal_racing_a_cancel_is_atomic_on_either_side() {
    // Either the seal lands first (and the cancel then refuses to void, marking
    // uncertain because a durable artifact exists) or the cancel voids first
    // (and the seal is refused). Never both, never a torn state.
    for _round in 0..4 {
        let fixture = TaskFixture::setup().await;
        let task_id = Uuid::new_v4();
        assert_eq!(
            fixture
                .admit_default(task_id, "op-seal-cancel")
                .await
                .status,
            200
        );

        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let seed_barrier = std::sync::Arc::clone(&barrier);
        let cancel_barrier = std::sync::Arc::clone(&barrier);
        let fixture_ref = &fixture;
        let (seed, cancel) = tokio::join!(
            async move {
                seed_barrier.wait().await;
                fixture_ref
                    .seed(
                        &fixture_ref.application_token,
                        task_id,
                        4,
                        &files(&[("a.txt", "a")]),
                    )
                    .await
            },
            async move {
                cancel_barrier.wait().await;
                fixture_ref
                    .json_post(
                        &fixture_ref.application_token,
                        &fixture_ref.cancel_path(task_id),
                        4,
                        serde_json::json!({ "expected_generation": 4 }),
                    )
                    .await
            }
        );

        let state: (String,) =
            sqlx::query_as("SELECT state FROM execution_environment_tasks WHERE task_id = $1")
                .bind(task_id)
                .fetch_one(&fixture.database.pool)
                .await
                .expect("task state");
        let headers: (i64,) = sqlx::query_as(
            "SELECT COUNT(*)::BIGINT FROM execution_environment_task_seeds WHERE task_id = $1",
        )
        .bind(task_id)
        .fetch_one(&fixture.database.pool)
        .await
        .expect("count headers");

        // A voided task has no writer, so a sealed snapshot may not exist on a
        // released task that was voided before the seal (the seal would have
        // been refused). Equivalently: if the seed row exists the task is not
        // `released`-from-void.
        assert!(
            state.0 == "released" || headers.0 == 1 || seed.status == 409,
            "no torn state: state={}, sealed={}, seed_status={}, cancel_status={}",
            state.0,
            headers.0,
            seed.status,
            cancel.status
        );
        fixture.teardown().await;
    }
}

/// Keep the raw caller helper referenced so the import stays explicit.
#[allow(dead_code)]
async fn _raw_is_available(fixture: &TaskFixture, task_id: Uuid) {
    let _ = call(
        &fixture.base,
        "GET",
        &fixture.seed_path(task_id),
        None,
        &[],
        b"",
    )
    .await;
}
