//! Janitor exclusion and deletion-audit cases (issue 753 P1).
//!
//! The janitor deletes archived workspace directories. A durable native task
//! owns its workspace for as long as it is active, so the sweep must skip it.
//! When it does delete, the audit row must retain the binding's identity: the
//! row is gone, so the foreign key is null, but `target_id` and the target type
//! still name what was removed. A silently skipped audit is a failure.

use super::admin_fixture::AdminFixture;
use super::support::create_code_project_binding;

#[tokio::test]
async fn the_janitor_skips_a_workspace_a_native_task_owns() {
    let admin = AdminFixture::setup().await;
    admin.admit_on(admin.other_binding_id, "op-janitor").await;
    admin.archive_for_janitor(admin.other_binding_id).await;

    admin.run_janitor().await;

    assert!(
        admin.binding_exists(admin.other_binding_id).await,
        "the janitor must not delete a workspace a native task owns"
    );
    assert!(
        std::path::Path::new(&admin.other_root)
            .join("keep.txt")
            .exists(),
        "the janitor must not remove files under an active task"
    );
    admin.fixture.teardown().await;
}

#[tokio::test]
async fn a_task_owned_workspace_keeps_its_binding_and_task_after_the_janitor_runs() {
    let admin = AdminFixture::setup().await;
    admin.admit_on(admin.other_binding_id, "op-keep").await;
    admin.archive_for_janitor(admin.other_binding_id).await;

    admin.run_janitor().await;

    // Nothing about the task or its workspace moved: the task is still the
    // same, no legacy lease was invented, and the files are intact.
    let task_state: (String,) = sqlx::query_as(
        "SELECT state FROM execution_environment_tasks WHERE workspace_binding_id = $1",
    )
    .bind(admin.other_binding_id)
    .fetch_one(&admin.fixture.database.pool)
    .await
    .expect("task row");
    assert_eq!(task_state.0, "admitted");
    let lease: (Option<String>,) = sqlx::query_as(
        "SELECT write_lease_owner FROM workspace_bindings WHERE workspace_binding_id = $1",
    )
    .bind(admin.other_binding_id)
    .fetch_one(&admin.fixture.database.pool)
    .await
    .expect("lease probe");
    assert_eq!(lease.0, None);
    assert!(
        std::path::Path::new(&admin.other_root)
            .join("keep.txt")
            .exists()
    );
    admin.fixture.teardown().await;
}

/// Create one archived, task-free workspace with a real file in it.
async fn orphan_workspace(admin: &AdminFixture) -> (i64, std::path::PathBuf) {
    let orphan_root = admin.admin_state.config.workspace_root.join("orphan");
    std::fs::create_dir_all(&orphan_root).expect("orphan workspace");
    std::fs::write(orphan_root.join("stale.txt"), b"stale").expect("seed orphan file");
    let orphan_binding = create_code_project_binding(
        &admin.fixture.database.pool,
        admin.fixture.application_id,
        &orphan_root.to_string_lossy(),
        "code_project:orphan",
    )
    .await;
    admin.archive_for_janitor(orphan_binding).await;
    (orphan_binding, orphan_root)
}

#[tokio::test]
async fn the_janitor_still_cleans_an_archived_workspace_with_no_task() {
    let admin = AdminFixture::setup().await;
    let (orphan_binding, orphan_root) = orphan_workspace(&admin).await;

    admin.run_janitor().await;

    assert!(
        !admin.binding_exists(orphan_binding).await,
        "an archived workspace with no task is still reclaimed"
    );
    assert!(
        !orphan_root.exists(),
        "an archived workspace with no task has its directory removed"
    );
    admin.fixture.teardown().await;
}

#[tokio::test]
async fn the_janitor_audit_retains_the_deleted_binding_identity() {
    let admin = AdminFixture::setup().await;
    let (orphan_binding, _root) = orphan_workspace(&admin).await;

    admin.run_janitor().await;

    // The binding row is gone, so the audit row cannot carry the foreign key —
    // but it must still name what was removed, exactly once. A missing or
    // silently skipped audit is a failure, not a cleanup detail.
    let rows: Vec<(String, String, String, Option<i64>)> = sqlx::query_as(
        "SELECT action, target_type, target_id, workspace_binding_id \
         FROM audit_log \
         WHERE action = 'workspace.delete' AND target_id = $1",
    )
    .bind(orphan_binding.to_string())
    .fetch_all(&admin.fixture.database.pool)
    .await
    .expect("audit rows");
    assert_eq!(rows.len(), 1, "exactly one deletion audit row");
    let (action, target_type, target_id, workspace_binding_id) = &rows[0];
    assert_eq!(action, "workspace.delete");
    assert_eq!(target_type, "workspace_binding");
    assert_eq!(target_id, &orphan_binding.to_string());
    assert_eq!(
        *workspace_binding_id, None,
        "the binding row is gone, so the FK cannot be retained"
    );
    admin.fixture.teardown().await;
}
