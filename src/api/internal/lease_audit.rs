//! Takeover audit records for the internal lease endpoint.
//!
//! Extracted from `lease.rs` to keep each file under the 400-line cap.
//! Covers the committed-takeover success record plus the quiescence-failure
//! record (no lease change, `resumed: false`, safe-to-retry).

use serde_json::json;

use crate::{
    AppState,
    api::internal::lease::LEASE_TAKEOVER_STALE_THRESHOLD_SECONDS,
    db::{
        queries,
        types::{WorkspaceLeaseTakeoverRequest, WorkspaceLeaseTakeoverResponse},
    },
    error::AppError,
};

pub(super) async fn record_takeover_audit(
    state: &AppState,
    application_id: i64,
    binding_id: i64,
    request: &WorkspaceLeaseTakeoverRequest,
    response: &WorkspaceLeaseTakeoverResponse,
) -> Result<(), AppError> {
    queries::record_audit(
        &state.db,
        crate::db::audit::AuditLogEntry {
            actor_user_id: None,
            actor_application_id: Some(application_id),
            actor_type: "application",
            action: "workspace.lease.takeover",
            target_type: "workspace_binding",
            target_id: &binding_id.to_string(),
            workspace_binding_id: Some(binding_id),
            external_user_id: None,
            payload: json!({
                "previous_owner": response.previous_owner,
                "previous_acquired_at": response.previous_acquired_at,
                "previous_expires_at": response.previous_expires_at,
                "new_owner": request.new_owner,
                "expected_owner": request.expected_owner,
                "force": request.force,
                "took_over_foreign": response.took_over_foreign,
                "granted_ttl_seconds": response.granted_ttl_seconds,
                "stale_threshold_seconds": response.stale_threshold_seconds,
                "cancellation": {
                    "attempted": response.cancellation.attempted,
                    "succeeded": response.cancellation.succeeded,
                    "sessions_cancelled": response.cancellation.sessions_cancelled,
                    "error": response.cancellation.error,
                }
            }),
            request_id: None,
            session_id: None,
            duration_ms: None,
            status: Some("success"),
        },
    )
    .await
    .map_err(AppError::internal)
}

/// Audit a foreign takeover that failed quiescence.
///
/// No lease change was committed, so the payload carries the still-current
/// lease owner for retry decisions and an explicit `resumed: false` marker:
/// no caller path may resume the new run after this outcome. Recorded with
/// status `failed` to distinguish it from committed takeovers.
pub(super) async fn record_takeover_quiescence_failure(
    state: &AppState,
    application_id: i64,
    binding_id: i64,
    request: &WorkspaceLeaseTakeoverRequest,
    current: &crate::db::types::WorkspaceLeaseStatusRow,
    cancellation: &crate::db::types::WorkspaceLeaseCancellationOutcome,
) -> Result<(), AppError> {
    queries::record_audit(
        &state.db,
        crate::db::audit::AuditLogEntry {
            actor_user_id: None,
            actor_application_id: Some(application_id),
            actor_type: "application",
            action: "workspace.lease.takeover",
            target_type: "workspace_binding",
            target_id: &binding_id.to_string(),
            workspace_binding_id: Some(binding_id),
            external_user_id: None,
            payload: json!({
                "new_owner": request.new_owner,
                "expected_owner": request.expected_owner,
                "force": request.force,
                "took_over_foreign": false,
                "resumed": false,
                "lease_changed": false,
                "current_owner": current.write_lease_owner,
                "stale_threshold_seconds": LEASE_TAKEOVER_STALE_THRESHOLD_SECONDS,
                "cancellation": {
                    "attempted": cancellation.attempted,
                    "succeeded": cancellation.succeeded,
                    "sessions_cancelled": cancellation.sessions_cancelled,
                    "error": cancellation.error,
                }
            }),
            request_id: None,
            session_id: None,
            duration_ms: None,
            status: Some("failed"),
        },
    )
    .await
    .map_err(AppError::internal)
}
