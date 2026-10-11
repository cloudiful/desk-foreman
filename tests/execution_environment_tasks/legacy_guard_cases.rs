//! Legacy-writer exclusion cases (issue 753 P1).
//!
//! A durable native task and a legacy workspace writer must never overlap.
//! These drive the real legacy entry points through the real internal HTTP
//! surface with local in-process fixtures — no live admin, no operator
//! surface.

use uuid::Uuid;

use super::lease_helpers::{acquire_lease, clear_legacy_owner};
use super::support::{TaskFixture, set_legacy_lease_owner};

#[tokio::test]
async fn an_active_native_task_refuses_every_legacy_lease_write() {
    let fixture = TaskFixture::setup().await;
    // No legacy owner yet: the acquire itself proves the guard fires only when
    // a task is active.
    assert_eq!(acquire_lease(&fixture, "before-task").await.status, 200);
    set_legacy_lease_owner(
        &fixture.database.pool,
        fixture.binding_id,
        "before-task",
        false,
    )
    .await;

    // A task cannot be admitted while a legacy owner exists.
    let blocked = fixture.admit_default(Uuid::new_v4(), "op-lease").await;
    assert_eq!(blocked.status, 409);

    // Clear the legacy owner, admit a task, then prove the legacy writer is
    // refused from every direction.
    clear_legacy_owner(&fixture).await;

    assert_eq!(
        fixture
            .admit_default(Uuid::new_v4(), "op-guard")
            .await
            .status,
        200
    );
    let refused = acquire_lease(&fixture, "after-task").await;
    assert_ne!(
        refused.status, 200,
        "a legacy write lease must not be granted over a native task"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_native_task_survives_legacy_transition_attempts() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    assert_eq!(
        fixture.admit_default(task_id, "op-survive").await.status,
        200
    );
    let (guard_tx, fresh_binding) = desk_foreman::db::queries::lock_workspace_binding_for_write(
        &fixture.database.pool,
        fixture.binding_id,
    )
    .await
    .expect("native guard query");
    assert!(
        fresh_binding.is_none(),
        "the locked native guard must refuse the binding"
    );
    drop(guard_tx);

    // Git sync for this workspace goes through the fenced write lock and must
    // be refused rather than dispatching.
    let sync = super::support::call(
        &fixture.base,
        "POST",
        "/api/internal/resource-workspaces/code_project/alpha/git/sync",
        Some(&fixture.application_token),
        &[
            super::support::fence_header(0),
            (
                "X-DF-Workspace-Key".to_string(),
                "code_project:alpha".to_string(),
            ),
        ],
        serde_json::to_vec(&serde_json::json!({
            "remote_url": "https://example.invalid/repo.git"
        }))
        .expect("body")
        .as_slice(),
    )
    .await;
    // 403 is the fenced-write refusal, so the request reached the guard rather
    // than failing earlier on routing or validation.
    assert_eq!(
        sync.status,
        403,
        "a native task must block the legacy git/sync writer: {}",
        sync.body_text()
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_task_does_not_disturb_the_legacy_binding_or_its_files() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    assert_eq!(fixture.admit_default(task_id, "op-files").await.status, 200);

    // The binding row itself is untouched by admission: same owner, same
    // lifecycle, no lease invented on the caller's behalf.
    let row: (String, String, bool, Option<String>) = sqlx::query_as(
        "SELECT external_user_id, lifecycle_state, is_active, write_lease_owner \
         FROM workspace_bindings WHERE workspace_binding_id = $1",
    )
    .bind(fixture.binding_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("binding row");
    assert_eq!(row.0, super::support::RESOURCE_MARKER);
    assert_eq!(row.1, "active");
    assert!(row.2);
    assert_eq!(
        row.3, None,
        "admission must never write a legacy lease owner"
    );
    fixture.teardown().await;
}
