//! Shared helpers used by the internal endpoint modules (lease, runner
//! manager, application). Kept in its own module so the per-domain
//! handler modules can stay focused and avoid duplication.

use axum::http::HeaderMap;
use serde_json::Value;

use crate::{AppState, db::types::WorkspaceBindingResponse, error::AppError};

pub(super) async fn application_actor(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<i64, AppError> {
    let Some(actor) = crate::auth::identity::mcp_actor_from_bearer(state, headers, false).await?
    else {
        return Err(AppError::unauthorized(
            "bearer token required for internal API",
        ));
    };
    let crate::actor::McpActor::ApplicationSubject { application, .. } = actor else {
        return Err(AppError::unauthorized(
            "application bearer token required for internal API",
        ));
    };
    Ok(application.application_id)
}

pub(super) async fn resolve_binding_for_application(
    state: &AppState,
    headers: &HeaderMap,
    binding_id: i64,
) -> Result<i64, AppError> {
    let application_id = application_actor(state, headers).await?;
    let binding = crate::db::queries::find_workspace_binding_by_id(&state.db, binding_id)
        .await?
        .ok_or_else(|| AppError::not_found("workspace binding not found"))?;
    if binding.application_id != application_id {
        return Err(AppError::forbidden(
            "workspace binding belongs to another application",
        ));
    }
    Ok(application_id)
}

/// Create-on-miss resource binding resolver. Preserves the existing POST
/// acquire/release resource-lease contract that materializes a resource
/// workspace on first access.
pub(super) async fn resolve_or_create_resource_binding_for_application(
    state: &AppState,
    headers: &HeaderMap,
    resource_kind: &str,
    resource_id: &str,
) -> Result<(i64, WorkspaceBindingResponse), AppError> {
    let application_id = application_actor(state, headers).await?;
    if !valid_resource_path_component(resource_kind) || !valid_resource_path_component(resource_id)
    {
        return Err(AppError::bad_request("invalid resource workspace key"));
    }
    let application = crate::db::queries::find_application_by_id(&state.db, application_id)
        .await?
        .ok_or_else(|| AppError::not_found("application not found"))?;
    let workspace_key = format!("{resource_kind}:{resource_id}");
    let binding = crate::auth::identity::resolve_or_create_resource_binding(
        state,
        &application,
        resource_kind,
        resource_id,
        &workspace_key,
    )
    .await?;
    Ok((application_id, binding))
}

/// Lookup-only resource binding resolver. Used by the lease-status GET and
/// the lease-takeover POST so neither endpoint materializes a workspace
/// directory or workspace_bindings row on first call; callers that need
/// create-on-miss semantics must use the existing POST acquire endpoint.
pub(super) async fn resolve_resource_binding_for_application(
    state: &AppState,
    headers: &HeaderMap,
    resource_kind: &str,
    resource_id: &str,
) -> Result<(i64, WorkspaceBindingResponse), AppError> {
    let application_id = application_actor(state, headers).await?;
    if !valid_resource_path_component(resource_kind) || !valid_resource_path_component(resource_id)
    {
        return Err(AppError::bad_request("invalid resource workspace key"));
    }
    let binding = crate::db::queries::find_workspace_binding_by_resource(
        &state.db,
        application_id,
        resource_kind,
        resource_id,
    )
    .await?
    .ok_or_else(|| AppError::not_found("workspace binding not found"))?;
    Ok((application_id, binding))
}

pub(super) fn valid_resource_path_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
}

/// Record a generic lease audit entry. Centralised so acquire and release
/// produce a uniform audit shape; takeover has its own helper because its
/// payload includes takeover-specific fields.
pub(super) async fn record_lease_audit(
    state: &AppState,
    application_id: i64,
    binding_id: i64,
    action: &'static str,
    payload: Value,
) -> Result<(), AppError> {
    crate::db::queries::record_audit(
        &state.db,
        crate::db::audit::AuditLogEntry {
            actor_user_id: None,
            actor_application_id: Some(application_id),
            actor_type: "application",
            action,
            target_type: "workspace_binding",
            target_id: &binding_id.to_string(),
            workspace_binding_id: Some(binding_id),
            external_user_id: None,
            payload,
            request_id: None,
            session_id: None,
            duration_ms: None,
            status: Some("success"),
        },
    )
    .await
    .map_err(AppError::internal)
}
