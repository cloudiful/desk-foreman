//! Guard parity across every workspace writer and lifecycle path.

use std::panic::AssertUnwindSafe;

use chrono::{Duration as ChronoDuration, Utc};
use desk_foreman::db::types::{TakeoverConflictReason, WorkspaceLeaseCancellationOutcome};
use desk_foreman::db::workspace_takeover::TakeoverOutcome;
use desk_foreman::db::{
    queries::{acquire_workspace_write_lease, lock_workspace_binding_for_write},
    workspace_takeover::acquire_workspace_write_lease_takeover,
};
use futures_util::FutureExt;
use uuid::Uuid;

use super::fixtures::{AdminHarness, Binding, Fixture, SeedJob};
use super::support::{IssueDb, SUITE_LOCK};

async fn seed(db: &IssueDb, fixture: &Fixture, binding: &Binding, state: &str) -> Uuid {
    fixture
        .seed_git_job(
            &db.pool,
            SeedJob {
                binding,
                state,
                reserved_bytes: 1,
                deadline_at: Utc::now() + ChronoDuration::minutes(5),
                claimed_at: if state == "pending" {
                    None
                } else {
                    Some(Utc::now())
                },
                cleanup_confirmed: false,
                terminal_at: None,
            },
        )
        .await
}

#[tokio::test]
async fn active_git_jobs_block_the_write_lock_lease_and_takeover() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let states = ["pending", "claimed", "uncertain"];
    let mut bindings = Vec::new();
    for (index, state) in states.iter().enumerate() {
        let binding = fixture
            .add_binding(&db.pool, &format!("code_project:guard-{index}"))
            .await;
        seed(&db, &fixture, &binding, state).await;
        bindings.push((state.to_string(), binding));
    }

    for (state, binding) in &bindings {
        let (tx, fresh) = lock_workspace_binding_for_write(&db.pool, binding.binding_id)
            .await
            .expect("write lock guard query");
        assert!(fresh.is_none(), "{state} must block the fenced write lock");
        drop(tx);

        let lease =
            acquire_workspace_write_lease(&db.pool, binding.binding_id, &fixture.lease_owner, 300)
                .await
                .expect("lease guard query");
        assert!(lease.is_none(), "{state} must block lease acquisition");

        let takeover = acquire_workspace_write_lease_takeover(
            &db.pool,
            binding.binding_id,
            "new-owner",
            300,
            &fixture.lease_owner,
            0,
            true,
            async { WorkspaceLeaseCancellationOutcome::default() },
        )
        .await
        .expect("takeover guard query");
        assert!(
            matches!(
                takeover,
                TakeoverOutcome::Conflict {
                    reason: TakeoverConflictReason::NotActive,
                    ..
                }
            ),
            "{state} must block takeover"
        );
    }

    // Native-only parity: with no Git job the same paths still admit.
    let clear = fixture
        .add_binding(&db.pool, "code_project:guard-clear")
        .await;
    let (tx, fresh) = lock_workspace_binding_for_write(&db.pool, clear.binding_id)
        .await
        .expect("clear write lock");
    assert!(fresh.is_some(), "a Git-free binding keeps its write path");
    drop(tx);
    let lease =
        acquire_workspace_write_lease(&db.pool, clear.binding_id, &fixture.lease_owner, 300)
            .await
            .expect("clear lease");
    assert!(lease.is_some(), "a Git-free binding keeps its lease path");

    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn admin_archive_refuses_every_active_git_state_and_still_archives_a_clear_binding() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let admin = AdminHarness::setup(db.pool.clone()).await;
    let outcome = {
        let body = async {
            let mut binding_ids = Vec::new();
            for (index, state) in ["pending", "claimed", "uncertain"].iter().enumerate() {
                let binding = fixture
                    .add_binding(&db.pool, &format!("code_project:admin-{index}"))
                    .await;
                seed(&db, &fixture, &binding, state).await;
                binding_ids.push(binding.binding_id);
            }
            let clear = fixture
                .add_binding(&db.pool, "code_project:admin-clear")
                .await;

            for binding_id in binding_ids {
                let status = admin.archive(binding_id).await;
                assert_eq!(status, 409, "archive must refuse an active Git job");
                let row =
                    desk_foreman::db::queries::find_workspace_binding_by_id(&db.pool, binding_id)
                        .await
                        .expect("read refused binding")
                        .expect("binding remains");
                assert_eq!(row.lifecycle_state, "active");
                assert!(row.is_active);
            }

            assert_eq!(
                admin.archive(clear.binding_id).await,
                200,
                "a Git-free binding still archives"
            );
            let row =
                desk_foreman::db::queries::find_workspace_binding_by_id(&db.pool, clear.binding_id)
                    .await
                    .expect("read archived binding")
                    .expect("binding remains");
            assert_eq!(row.lifecycle_state, "archived");
        };
        AssertUnwindSafe(body).catch_unwind().await
    };
    admin.cleanup(&db.pool).await;
    fixture.cleanup(&db.pool).await;
    if let Err(payload) = outcome {
        std::panic::resume_unwind(payload);
    }
}

#[tokio::test]
async fn the_janitor_skips_every_active_git_state() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let admin = AdminHarness::setup(db.pool.clone()).await;
    let outcome = {
        let body = async {
            let mut binding_ids = Vec::new();
            for (index, state) in ["pending", "claimed", "uncertain"].iter().enumerate() {
                let binding = fixture
                    .add_binding(&db.pool, &format!("code_project:janitor-{index}"))
                    .await;
                seed(&db, &fixture, &binding, state).await;
                Fixture::archive_for_janitor(&db.pool, binding.binding_id).await;
                binding_ids.push(binding.binding_id);
            }

            desk_foreman::lifecycle::cleanup_archived_workspaces(admin.state())
                .await
                .expect("janitor sweep");

            for binding_id in binding_ids {
                assert!(
                    desk_foreman::db::queries::find_workspace_binding_by_id(&db.pool, binding_id)
                        .await
                        .expect("probe janitor binding")
                        .is_some(),
                    "the janitor must skip a binding with an active Git job"
                );
            }
        };
        AssertUnwindSafe(body).catch_unwind().await
    };
    admin.cleanup(&db.pool).await;
    fixture.cleanup(&db.pool).await;
    if let Err(payload) = outcome {
        std::panic::resume_unwind(payload);
    }
}
