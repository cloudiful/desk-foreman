//! Legacy lease regression and race cases (issue 753 P1).
//!
//! Split from `legacy_guard_cases.rs` so no test file exceeds the size budget.
//! These prove the guard is additive — the no-native path is unchanged — and
//! that a native task refuses the legacy release and force-takeover, including
//! under a genuine race with a legacy acquire.

use uuid::Uuid;

use super::lease_helpers::{
    acquire_lease, clear_legacy_owner, force_takeover_lease, release_lease,
};
use super::support::{TaskFixture, set_legacy_lease_owner};

#[tokio::test]
async fn a_legacy_workspace_with_no_task_behaves_exactly_as_before() {
    let fixture = TaskFixture::setup().await;
    // No task admitted: the legacy writer still works, proving the guard is
    // additive and not a behaviour change for the old path.
    assert_eq!(acquire_lease(&fixture, "legacy-owner").await.status, 200);
    let stored: (Option<String>,) = sqlx::query_as(
        "SELECT write_lease_owner FROM workspace_bindings WHERE workspace_binding_id = $1",
    )
    .bind(fixture.binding_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("binding row");
    assert_eq!(stored.0.as_deref(), Some("legacy-owner"));
    fixture.teardown().await;
}

#[tokio::test]
async fn a_concurrent_admission_and_legacy_acquire_produce_one_winner() {
    // A genuine race: both racers leave the barrier at the same instant and hit
    // their real entry points. Whichever commits first, the other must be
    // refused rather than stepping through a stale snapshot.
    for _round in 0..8 {
        let fixture = TaskFixture::setup().await;
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));

        let admit_barrier = std::sync::Arc::clone(&barrier);
        let acquire_barrier = std::sync::Arc::clone(&barrier);
        // Both futures borrow the fixture, so neither takes ownership of it.
        let admitting: &TaskFixture = &fixture;
        let acquiring: &TaskFixture = &fixture;
        let (admit_status, acquire_status) = tokio::join!(
            async move {
                admit_barrier.wait().await;
                admitting
                    .admit_default(Uuid::new_v4(), "op-race")
                    .await
                    .status
            },
            async move {
                acquire_barrier.wait().await;
                acquire_lease(acquiring, "race-owner").await.status
            }
        );

        let task_active: (i64,) = sqlx::query_as(
            "SELECT COUNT(*)::BIGINT FROM execution_environment_tasks \
             WHERE state IN ('admitted','preparing','running','stopping','uncertain')",
        )
        .fetch_one(&fixture.database.pool)
        .await
        .expect("count");
        let lease_owner: (Option<String>,) = sqlx::query_as(
            "SELECT write_lease_owner FROM workspace_bindings WHERE workspace_binding_id = $1",
        )
        .bind(fixture.binding_id)
        .fetch_one(&fixture.database.pool)
        .await
        .expect("binding");

        // Exactly one racer may win: an admitted task and a legacy write owner
        // can never coexist, whichever committed first.
        assert!(
            (task_active.0 == 1) ^ lease_owner.0.is_some(),
            "exactly one winner expected \
             (admit={admit_status}, acquire={acquire_status}, \
             tasks={}, owner={:?})",
            task_active.0,
            lease_owner.0
        );
        fixture.teardown().await;
    }
}

#[tokio::test]
async fn a_native_task_refuses_release_and_force_takeover_of_the_legacy_lease() {
    let fixture = TaskFixture::setup().await;
    // A stale legacy owner, then cleared so a task can be admitted.
    set_legacy_lease_owner(
        &fixture.database.pool,
        fixture.binding_id,
        "stale-owner",
        true,
    )
    .await;
    clear_legacy_owner(&fixture).await;

    let task_id = Uuid::new_v4();
    assert_eq!(
        fixture
            .admit_default(task_id, "op-release-takeover")
            .await
            .status,
        200
    );

    // Force is not a way past a native task: however stale the legacy lease
    // looks, the takeover must refuse.
    let takeover = force_takeover_lease(&fixture, "stale-owner").await;
    assert_ne!(
        takeover.status,
        200,
        "force takeover must not step past a native task: {}",
        takeover.body_text()
    );
    let taken: (Option<String>,) = sqlx::query_as(
        "SELECT write_lease_owner FROM workspace_bindings WHERE workspace_binding_id = $1",
    )
    .bind(fixture.binding_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("lease probe");
    assert_ne!(
        taken.0.as_deref(),
        Some("takeover-owner"),
        "a refused takeover must not install a new legacy owner"
    );

    // Release is refused too, and the task is never disturbed by it.
    let release = release_lease(&fixture, "stale-owner").await;
    assert_ne!(
        release.status,
        200,
        "release must not act on a workspace a native task owns: {}",
        release.body_text()
    );
    let task_state: (String,) =
        sqlx::query_as("SELECT state FROM execution_environment_tasks WHERE task_id = $1")
            .bind(task_id)
            .fetch_one(&fixture.database.pool)
            .await
            .expect("task row");
    assert_eq!(task_state.0, "admitted");
    fixture.teardown().await;
}

#[tokio::test]
async fn release_and_takeover_still_work_with_no_native_task() {
    let fixture = TaskFixture::setup().await;
    assert_eq!(acquire_lease(&fixture, "legacy-owner").await.status, 200);

    let takeover = force_takeover_lease(&fixture, "legacy-owner").await;
    assert_eq!(
        takeover.status,
        200,
        "with no native task the legacy takeover is unchanged: {}",
        takeover.body_text()
    );

    let release = release_lease(&fixture, "takeover-owner").await;
    assert_eq!(
        release.status,
        200,
        "with no native task the legacy release is unchanged: {}",
        release.body_text()
    );
    let stored: (Option<String>,) = sqlx::query_as(
        "SELECT write_lease_owner FROM workspace_bindings WHERE workspace_binding_id = $1",
    )
    .bind(fixture.binding_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("lease probe");
    assert_eq!(stored.0, None);
    fixture.teardown().await;
}
