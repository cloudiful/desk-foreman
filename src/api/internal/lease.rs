//! Internal lease endpoints.
//!
//! Contains the dedicated write-lease takeover and status endpoints plus the
//! ordinary acquire/release endpoints that share the same resource binding
//! scope. The takeover handler drives the transaction-based atomic state
//! machine in [`crate::db::workspace_bindings::acquire_workspace_write_lease_takeover`].

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use axum_extra::extract::cookie::CookieJar;
use serde_json::json;

use crate::{
    AppState,
    api::{
        admin::{
            lease_helpers::cancel_binding_sessions_best_effort,
            workspace_bindings::{acquire_binding_lease, release_binding_lease},
        },
        internal::shared::{
            record_lease_audit, resolve_binding_for_application,
            resolve_or_create_resource_binding_for_application,
            resolve_resource_binding_for_application, valid_resource_path_component,
        },
        validation::ValidatedJson,
    },
    db::{
        queries::{self, TakeoverOutcome},
        types::{
            WorkspaceBindingResponse, WorkspaceLeaseCancellationOutcome,
            WorkspaceLeaseReleaseRequest, WorkspaceLeaseRequest, WorkspaceLeaseStatusResponse,
            WorkspaceLeaseTakeoverConflict, WorkspaceLeaseTakeoverRequest,
            WorkspaceLeaseTakeoverResponse,
        },
    },
    error::AppError,
};

/// Stale guard used by the atomic write-lease takeover endpoint.
///
/// A foreign lease is eligible for takeover only when its last
/// `write_lease_acquired_at` is at least this many seconds older than the
/// database clock at lock time. The threshold is a fixed server constant;
/// callers may not choose a shorter value.
pub(crate) const LEASE_TAKEOVER_STALE_THRESHOLD_SECONDS: u64 = 180;

/// TTL granted to a successful takeover (or same-owner renew). Server
/// constant to keep contract behavior deterministic for stock callers.
pub(crate) const LEASE_TAKEOVER_GRANTED_TTL_SECONDS: u64 = 600;

#[utoipa::path(
    post,
    path = "/api/internal/workspace-bindings/{binding_id}/lease",
    tag = "internal",
    params(("binding_id" = i64, Path, description = "Workspace binding identifier")),
    request_body = WorkspaceLeaseRequest,
    responses(
        (status = 200, body = WorkspaceBindingResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse)
    )
)]
pub async fn acquire_workspace_binding_lease(
    State(state): State<AppState>,
    _jar: CookieJar,
    headers: HeaderMap,
    Path(binding_id): Path<i64>,
    ValidatedJson(request): ValidatedJson<WorkspaceLeaseRequest>,
) -> Result<Json<WorkspaceBindingResponse>, AppError> {
    let application_id = resolve_binding_for_application(&state, &headers, binding_id).await?;
    let binding =
        acquire_binding_lease(&state, binding_id, &request.owner, request.ttl_seconds).await?;
    record_lease_audit(
        &state,
        application_id,
        binding_id,
        "workspace.lease.acquire",
        json!({ "owner": request.owner, "ttl_seconds": request.ttl_seconds }),
    )
    .await?;
    Ok(Json(binding))
}

#[utoipa::path(
    delete,
    path = "/api/internal/workspace-bindings/{binding_id}/lease",
    tag = "internal",
    params(("binding_id" = i64, Path, description = "Workspace binding identifier")),
    request_body = WorkspaceLeaseReleaseRequest,
    responses(
        (status = 200, body = WorkspaceBindingResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse)
    )
)]
pub async fn release_workspace_binding_lease(
    State(state): State<AppState>,
    _jar: CookieJar,
    headers: HeaderMap,
    Path(binding_id): Path<i64>,
    ValidatedJson(request): ValidatedJson<WorkspaceLeaseReleaseRequest>,
) -> Result<Json<WorkspaceBindingResponse>, AppError> {
    let application_id = resolve_binding_for_application(&state, &headers, binding_id).await?;
    let binding = release_binding_lease(&state, binding_id, &request.owner).await?;
    record_lease_audit(
        &state,
        application_id,
        binding_id,
        "workspace.lease.release",
        json!({ "owner": request.owner }),
    )
    .await?;
    Ok(Json(binding))
}

#[utoipa::path(
    post,
    path = "/api/internal/resource-workspaces/{resource_kind}/{resource_id}/lease",
    tag = "internal",
    params(
        ("resource_kind" = String, Path, description = "Resource workspace kind"),
        ("resource_id" = String, Path, description = "Resource workspace identifier")
    ),
    request_body = WorkspaceLeaseRequest,
    responses(
        (status = 200, body = WorkspaceBindingResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse)
    )
)]
pub async fn acquire_resource_workspace_lease(
    State(state): State<AppState>,
    _jar: CookieJar,
    headers: HeaderMap,
    Path((resource_kind, resource_id)): Path<(String, String)>,
    ValidatedJson(request): ValidatedJson<WorkspaceLeaseRequest>,
) -> Result<Json<WorkspaceBindingResponse>, AppError> {
    let (application_id, binding) = resolve_or_create_resource_binding_for_application(
        &state,
        &headers,
        &resource_kind,
        &resource_id,
    )
    .await?;
    let binding = acquire_binding_lease(
        &state,
        binding.workspace_binding_id,
        &request.owner,
        request.ttl_seconds,
    )
    .await?;
    record_lease_audit(
        &state,
        application_id,
        binding.workspace_binding_id,
        "workspace.lease.acquire",
        json!({ "owner": request.owner, "ttl_seconds": request.ttl_seconds }),
    )
    .await?;
    Ok(Json(binding))
}

#[utoipa::path(
    delete,
    path = "/api/internal/resource-workspaces/{resource_kind}/{resource_id}/lease",
    tag = "internal",
    params(
        ("resource_kind" = String, Path, description = "Resource workspace kind"),
        ("resource_id" = String, Path, description = "Resource workspace identifier")
    ),
    request_body = WorkspaceLeaseReleaseRequest,
    responses(
        (status = 200, body = WorkspaceBindingResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse)
    )
)]
pub async fn release_resource_workspace_lease(
    State(state): State<AppState>,
    _jar: CookieJar,
    headers: HeaderMap,
    Path((resource_kind, resource_id)): Path<(String, String)>,
    ValidatedJson(request): ValidatedJson<WorkspaceLeaseReleaseRequest>,
) -> Result<Json<WorkspaceBindingResponse>, AppError> {
    let (application_id, binding) = resolve_or_create_resource_binding_for_application(
        &state,
        &headers,
        &resource_kind,
        &resource_id,
    )
    .await?;
    let binding =
        release_binding_lease(&state, binding.workspace_binding_id, &request.owner).await?;
    record_lease_audit(
        &state,
        application_id,
        binding.workspace_binding_id,
        "workspace.lease.release",
        json!({ "owner": request.owner }),
    )
    .await?;
    Ok(Json(binding))
}

#[utoipa::path(
    post,
    path = "/api/internal/resource-workspaces/{resource_kind}/{resource_id}/lease/takeover",
    tag = "internal",
    params(
        ("resource_kind" = String, Path, description = "Resource workspace kind"),
        ("resource_id" = String, Path, description = "Resource workspace identifier")
    ),
    request_body = WorkspaceLeaseTakeoverRequest,
    responses(
        (status = 200, body = WorkspaceLeaseTakeoverResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = WorkspaceLeaseTakeoverConflict)
    )
)]
pub async fn takeover_resource_workspace_lease(
    State(state): State<AppState>,
    _jar: CookieJar,
    headers: HeaderMap,
    Path((resource_kind, resource_id)): Path<(String, String)>,
    ValidatedJson(request): ValidatedJson<WorkspaceLeaseTakeoverRequest>,
) -> Result<Json<WorkspaceLeaseTakeoverResponse>, AppError> {
    if !valid_resource_path_component(&resource_kind)
        || !valid_resource_path_component(&resource_id)
    {
        return Err(AppError::bad_request("invalid resource workspace key"));
    }
    let (application_id, binding) =
        resolve_resource_binding_for_application(&state, &headers, &resource_kind, &resource_id)
            .await?;
    let binding_id = binding.workspace_binding_id;

    let outcome = queries::acquire_workspace_write_lease_takeover(
        &state.db,
        binding_id,
        &request.new_owner,
        LEASE_TAKEOVER_GRANTED_TTL_SECONDS,
        &request.expected_owner,
        LEASE_TAKEOVER_STALE_THRESHOLD_SECONDS,
    )
    .await?;

    match outcome {
        TakeoverOutcome::Success {
            binding,
            previous_owner,
            previous_acquired_at,
            previous_expires_at,
            took_over_foreign,
        } => {
            let cancellation = if took_over_foreign {
                cancel_binding_sessions_best_effort(&state, binding_id).await
            } else {
                WorkspaceLeaseCancellationOutcome::default()
            };
            let response = WorkspaceLeaseTakeoverResponse {
                binding,
                previous_owner,
                previous_acquired_at,
                previous_expires_at,
                took_over_foreign,
                granted_ttl_seconds: LEASE_TAKEOVER_GRANTED_TTL_SECONDS,
                stale_threshold_seconds: LEASE_TAKEOVER_STALE_THRESHOLD_SECONDS,
                cancellation,
            };
            record_takeover_audit(&state, application_id, binding_id, &request, &response).await?;
            Ok(Json(response))
        }
        TakeoverOutcome::Conflict { reason, current } => {
            let reason_str = reason.as_str().to_string();
            Err(AppError::TakeoverConflict(WorkspaceLeaseTakeoverConflict {
                error: conflict_message(reason),
                reason: reason_str,
                stale_threshold_seconds: LEASE_TAKEOVER_STALE_THRESHOLD_SECONDS,
                current: WorkspaceLeaseStatusResponse {
                    stale_threshold_seconds: LEASE_TAKEOVER_STALE_THRESHOLD_SECONDS,
                    ..current.into()
                },
            }))
        }
        TakeoverOutcome::NotFound => Err(AppError::not_found("workspace binding not found")),
    }
}

#[utoipa::path(
    get,
    path = "/api/internal/resource-workspaces/{resource_kind}/{resource_id}/lease",
    tag = "internal",
    params(
        ("resource_kind" = String, Path, description = "Resource workspace kind"),
        ("resource_id" = String, Path, description = "Resource workspace identifier")
    ),
    responses(
        (status = 200, body = WorkspaceLeaseStatusResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse)
    )
)]
pub async fn resource_workspace_lease_status(
    State(state): State<AppState>,
    _jar: CookieJar,
    headers: HeaderMap,
    Path((resource_kind, resource_id)): Path<(String, String)>,
) -> Result<Json<WorkspaceLeaseStatusResponse>, AppError> {
    let (_application_id, binding) =
        resolve_resource_binding_for_application(&state, &headers, &resource_kind, &resource_id)
            .await?;
    let mut status: WorkspaceLeaseStatusResponse = queries::find_active_resource_workspace_lease(
        &state.db,
        binding.application_id,
        &resource_kind,
        &resource_id,
    )
    .await?
    .ok_or_else(|| AppError::not_found("workspace binding not found"))?
    .into();
    status.stale_threshold_seconds = LEASE_TAKEOVER_STALE_THRESHOLD_SECONDS;
    Ok(Json(status))
}

fn conflict_message(reason: crate::db::types::TakeoverConflictReason) -> String {
    use crate::db::types::TakeoverConflictReason;
    match reason {
        TakeoverConflictReason::NoLease => {
            "no foreign lease to take over; use the ordinary acquire endpoint".to_string()
        }
        TakeoverConflictReason::LiveLease => {
            "lease is still within the stale window; cannot take over yet".to_string()
        }
        TakeoverConflictReason::ExpectedOwnerMismatch => {
            "current lease owner does not match expected_owner".to_string()
        }
        TakeoverConflictReason::NotActive => "workspace binding is not active".to_string(),
    }
}

async fn record_takeover_audit(
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
