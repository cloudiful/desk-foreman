//! Legacy write-lease route helpers for the native-task suite (issue 753 P1).
//!
//! Split so the guard cases and the no-native regression cases share one
//! definition of each legacy entry point instead of duplicating the wire shape.

use super::support::{HttpOutcome, TaskFixture, call, fence_header};

/// Acquire the legacy write lease through the internal lease route.
pub async fn acquire_lease(fixture: &TaskFixture, owner: &str) -> HttpOutcome {
    call(
        &fixture.base,
        "POST",
        &format!(
            "/api/internal/workspace-bindings/{}/lease",
            fixture.binding_id
        ),
        Some(&fixture.application_token),
        &[fence_header(0)],
        serde_json::to_vec(&serde_json::json!({ "owner": owner, "ttl_seconds": 600 }))
            .expect("body")
            .as_slice(),
    )
    .await
}

/// Release the legacy write lease through the internal lease route.
pub async fn release_lease(fixture: &TaskFixture, owner: &str) -> HttpOutcome {
    call(
        &fixture.base,
        "DELETE",
        &format!(
            "/api/internal/workspace-bindings/{}/lease",
            fixture.binding_id
        ),
        Some(&fixture.application_token),
        &[fence_header(0)],
        serde_json::to_vec(&serde_json::json!({ "owner": owner }))
            .expect("body")
            .as_slice(),
    )
    .await
}

/// Force-take the legacy write lease through the internal resource route.
pub async fn force_takeover_lease(fixture: &TaskFixture, owner: &str) -> HttpOutcome {
    call(
        &fixture.base,
        "POST",
        "/api/internal/resource-workspaces/code_project/alpha/lease/takeover",
        Some(&fixture.application_token),
        &[fence_header(0)],
        serde_json::to_vec(&serde_json::json!({
            "expected_owner": owner,
            "new_owner": "takeover-owner",
            "force": true,
        }))
        .expect("body")
        .as_slice(),
    )
    .await
}

/// Clear any legacy write owner so a native intent can be admitted.
pub async fn clear_legacy_owner(fixture: &TaskFixture) {
    sqlx::query(
        "UPDATE workspace_bindings SET write_lease_owner = NULL, write_lease_acquired_at = NULL, \
         write_lease_expires_at = NULL WHERE workspace_binding_id = $1",
    )
    .bind(fixture.binding_id)
    .execute(&fixture.database.pool)
    .await
    .expect("clear legacy owner");
}
