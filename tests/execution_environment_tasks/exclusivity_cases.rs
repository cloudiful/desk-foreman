//! Admission exclusivity and eligibility cases (issue 753 P1).
//!
//! Split from `admission_cases.rs` so no test file exceeds the size budget.
//! These cover what an admitted task excludes: the per-environment and
//! per-binding active slots, the legacy write-lease owner, and the binding
//! resource-kind/lifecycle eligibility rules.

use uuid::Uuid;

use super::support::{TaskFixture, call, set_legacy_lease_owner};

#[tokio::test]
async fn a_second_task_cannot_share_the_environment_or_the_binding() {
    let fixture = TaskFixture::setup().await;
    let first = Uuid::new_v4();
    assert_eq!(fixture.admit_default(first, "op-first").await.status, 200);

    // Same environment, different task: refused.
    let same_env = fixture.admit_default(Uuid::new_v4(), "op-same-env").await;
    assert_eq!(same_env.status, 409, "one active task per environment");

    // A second, independent workspace on the same environment is still
    // refused: the environment anchor is the exclusive slot.
    let other_binding = super::support::create_code_project_binding(
        &fixture.database.pool,
        fixture.application_id,
        "/tmp/other-workspace",
        "code_project:beta",
    )
    .await;
    let other_workspace = fixture
        .admit(
            &fixture.application_token,
            Uuid::new_v4(),
            other_binding,
            "task-user",
            4,
            "op-other-ws",
        )
        .await;
    assert_eq!(
        other_workspace.status, 409,
        "one active task per environment, whatever the workspace"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_binding_held_by_another_environment_is_exclusive() {
    let fixture = TaskFixture::setup().await;
    assert_eq!(
        fixture
            .admit_default(Uuid::new_v4(), "op-holder")
            .await
            .status,
        200
    );
    // A second environment of the same application pointing at the same
    // workspace: the binding is shared, so the task is refused.
    let second_env = super::support::create_environment(
        &fixture.database.pool,
        fixture.application_id,
        "second-user",
    )
    .await;
    super::support::set_idle_fence(&fixture.database.pool, second_env, fixture.manager_id, 4).await;
    let outcome = call(
        &fixture.base,
        "POST",
        &format!("/api/internal/execution-environments/{second_env}/tasks/admit"),
        Some(&fixture.application_token),
        &[super::support::fence_header(4)],
        serde_json::to_vec(&serde_json::json!({
            "task_id": Uuid::new_v4(),
            "workspace_binding_id": fixture.binding_id,
            "subject_external_user_id": "second-user",
            "expected_generation": 4,
            "operation_token": "op-second-env",
        }))
        .expect("body")
        .as_slice(),
    )
    .await;
    assert_eq!(
        outcome.status, 409,
        "a shared workspace carries at most one active task"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn any_legacy_lease_owner_blocks_admission_even_when_expired() {
    let fixture = TaskFixture::setup().await;
    // Expired by the clock, still a real owner: an expiry is a cleanup timer,
    // never evidence that no dispatched process is writing.
    set_legacy_lease_owner(
        &fixture.database.pool,
        fixture.binding_id,
        "session-1",
        true,
    )
    .await;
    let expired = fixture.admit_default(Uuid::new_v4(), "op-expired").await;
    assert_eq!(expired.status, 409, "an expired owner still blocks");
    assert!(
        !expired.body_text().contains("session-1"),
        "the refusal must not echo the owner"
    );

    // A live owner blocks the same way.
    set_legacy_lease_owner(
        &fixture.database.pool,
        fixture.binding_id,
        "session-2",
        false,
    )
    .await;
    assert_eq!(
        fixture
            .admit_default(Uuid::new_v4(), "op-live")
            .await
            .status,
        409
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn an_inactive_or_non_code_project_binding_is_refused() {
    let fixture = TaskFixture::setup().await;
    sqlx::query(
        "UPDATE workspace_bindings SET resource_kind = 'other_thing' WHERE workspace_binding_id = $1",
    )
    .bind(fixture.binding_id)
    .execute(&fixture.database.pool)
    .await
    .expect("change resource kind");
    assert_eq!(
        fixture
            .admit_default(Uuid::new_v4(), "op-kind")
            .await
            .status,
        409,
        "only a code project workspace may carry a native task"
    );

    sqlx::query(
        "UPDATE workspace_bindings SET resource_kind = 'code_project', is_active = FALSE WHERE workspace_binding_id = $1",
    )
    .bind(fixture.binding_id)
    .execute(&fixture.database.pool)
    .await
    .expect("deactivate binding");
    assert_eq!(
        fixture
            .admit_default(Uuid::new_v4(), "op-inactive")
            .await
            .status,
        409
    );
    fixture.teardown().await;
}
