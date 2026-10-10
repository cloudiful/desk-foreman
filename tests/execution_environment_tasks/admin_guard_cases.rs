//! Admin-transition exclusion cases (issue 753 P1).
//!
//! A durable native task owns its workspace outright, so archiving, restoring
//! or resetting that workspace must refuse while the task is active — and must
//! leave the task, the binding row and the workspace files exactly as they
//! were. Admin fixtures are local and in process; no live admin or operator
//! surface is involved.

use super::admin_fixture::AdminFixture;
use super::support::set_legacy_lease_owner;

#[tokio::test]
async fn archiving_a_workspace_a_native_task_owns_is_refused() {
    let admin = AdminFixture::setup().await;
    admin.admit_on(admin.other_binding_id, "op-archive").await;

    let outcome = admin
        .admin_post(&format!(
            "/api/admin/workspace-bindings/{}/archive",
            admin.other_binding_id
        ))
        .await;
    assert_eq!(
        outcome.status,
        409,
        "archive must refuse while a task owns the workspace: {}",
        outcome.body_text()
    );
    assert_eq!(
        admin.binding_state(admin.other_binding_id).await,
        ("active".to_string(), true),
        "a refused archive must not move the binding"
    );
    admin.fixture.teardown().await;
}

#[tokio::test]
async fn restoring_and_resetting_a_task_owned_workspace_are_refused() {
    let admin = AdminFixture::setup().await;
    admin
        .admit_on(admin.other_binding_id, "op-restore-reset")
        .await;

    let restore = admin
        .admin_post(&format!(
            "/api/admin/workspace-bindings/{}/restore",
            admin.other_binding_id
        ))
        .await;
    assert_eq!(restore.status, 409, "restore: {}", restore.body_text());

    let reset = admin
        .admin_post(&format!(
            "/api/admin/workspace-bindings/{}/reset",
            admin.other_binding_id
        ))
        .await;
    assert_eq!(
        reset.status,
        409,
        "reset destroys the directory, so it must refuse: {}",
        reset.body_text()
    );

    // The refusal is real: neither the row nor the workspace files moved.
    assert_eq!(
        admin.binding_state(admin.other_binding_id).await,
        ("active".to_string(), true)
    );
    assert!(
        std::path::Path::new(&admin.other_root)
            .join("keep.txt")
            .exists(),
        "a refused reset must not destroy the workspace directory"
    );
    admin.fixture.teardown().await;
}

#[tokio::test]
async fn an_admin_transition_still_works_when_no_task_owns_the_workspace() {
    let admin = AdminFixture::setup().await;
    // No task: the guard is additive and must not change the old behaviour.
    let archive = admin
        .admin_post(&format!(
            "/api/admin/workspace-bindings/{}/archive",
            admin.other_binding_id
        ))
        .await;
    assert_eq!(archive.status, 200, "archive: {}", archive.body_text());
    assert_eq!(
        admin.binding_state(admin.other_binding_id).await,
        ("archived".to_string(), false)
    );

    let restore = admin
        .admin_post(&format!(
            "/api/admin/workspace-bindings/{}/restore",
            admin.other_binding_id
        ))
        .await;
    assert_eq!(restore.status, 200, "restore: {}", restore.body_text());
    assert_eq!(
        admin.binding_state(admin.other_binding_id).await,
        ("active".to_string(), true)
    );
    admin.fixture.teardown().await;
}

#[tokio::test]
async fn an_expired_legacy_owner_is_not_a_native_conflict() {
    let admin = AdminFixture::setup().await;
    // The guard must be a no-op for every binding without a native task: an
    // expired legacy lease is a cleanup timer, never a native conflict.
    set_legacy_lease_owner(
        &admin.fixture.database.pool,
        admin.other_binding_id,
        "legacy-session",
        true,
    )
    .await;
    let archive = admin
        .admin_post(&format!(
            "/api/admin/workspace-bindings/{}/archive",
            admin.other_binding_id
        ))
        .await;
    assert_eq!(
        archive.status,
        200,
        "an expired legacy lease is not a native task: {}",
        archive.body_text()
    );
    admin.fixture.teardown().await;
}
