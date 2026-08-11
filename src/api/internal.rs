use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use axum_extra::extract::cookie::CookieJar;
use serde_json::json;

use crate::{
    AppState,
    api::validation::ValidatedJson,
    db::types::{WorkspaceLeaseReleaseRequest, WorkspaceLeaseRequest},
    error::AppError,
};

use super::admin::workspace_bindings::{acquire_binding_lease, release_binding_lease};

pub(super) fn router() -> axum::Router<AppState> {
    axum::Router::new()
        .route(
            "/api/internal/workspace-bindings/{binding_id}/lease",
            axum::routing::post(acquire_workspace_binding_lease)
                .delete(release_workspace_binding_lease),
        )
        .route(
            "/api/internal/resource-workspaces/{resource_kind}/{resource_id}/lease",
            axum::routing::post(acquire_resource_workspace_lease)
                .delete(release_resource_workspace_lease),
        )
}

async fn application_actor(state: &AppState, headers: &HeaderMap) -> Result<i64, AppError> {
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

async fn resolve_binding_for_application(
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

async fn resolve_or_create_resource_binding_for_application(
    state: &AppState,
    headers: &HeaderMap,
    resource_kind: &str,
    resource_id: &str,
) -> Result<(i64, crate::db::types::WorkspaceBindingResponse), AppError> {
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

fn valid_resource_path_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
}

#[utoipa::path(
    post,
    path = "/api/internal/workspace-bindings/{binding_id}/lease",
    tag = "internal",
    params(("binding_id" = i64, Path, description = "Workspace binding identifier")),
    request_body = WorkspaceLeaseRequest,
    responses(
        (status = 200, body = crate::db::types::WorkspaceBindingResponse),
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
) -> Result<Json<crate::db::types::WorkspaceBindingResponse>, AppError> {
    let application_id = resolve_binding_for_application(&state, &headers, binding_id).await?;
    acquire_binding_lease(&state, binding_id, &request.owner, request.ttl_seconds).await?;
    crate::db::queries::record_audit(
        &state.db,
        crate::db::audit::AuditLogEntry {
            actor_user_id: None,
            actor_application_id: Some(application_id),
            actor_type: "application",
            action: "workspace.lease.acquire",
            target_type: "workspace_binding",
            target_id: &binding_id.to_string(),
            workspace_binding_id: Some(binding_id),
            external_user_id: None,
            payload: json!({ "owner": request.owner, "ttl_seconds": request.ttl_seconds }),
            request_id: None,
            session_id: None,
            duration_ms: None,
            status: Some("success"),
        },
    )
    .await?;
    let binding = crate::db::queries::find_workspace_binding_by_id(&state.db, binding_id)
        .await?
        .ok_or_else(|| AppError::not_found("workspace binding not found"))?;
    Ok(Json(binding))
}

#[utoipa::path(
    delete,
    path = "/api/internal/workspace-bindings/{binding_id}/lease",
    tag = "internal",
    params(("binding_id" = i64, Path, description = "Workspace binding identifier")),
    request_body = WorkspaceLeaseReleaseRequest,
    responses(
        (status = 200, body = crate::db::types::WorkspaceBindingResponse),
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
) -> Result<Json<crate::db::types::WorkspaceBindingResponse>, AppError> {
    let application_id = resolve_binding_for_application(&state, &headers, binding_id).await?;
    release_binding_lease(&state, binding_id, &request.owner).await?;
    crate::db::queries::record_audit(
        &state.db,
        crate::db::audit::AuditLogEntry {
            actor_user_id: None,
            actor_application_id: Some(application_id),
            actor_type: "application",
            action: "workspace.lease.release",
            target_type: "workspace_binding",
            target_id: &binding_id.to_string(),
            workspace_binding_id: Some(binding_id),
            external_user_id: None,
            payload: json!({ "owner": request.owner }),
            request_id: None,
            session_id: None,
            duration_ms: None,
            status: Some("success"),
        },
    )
    .await?;
    let binding = crate::db::queries::find_workspace_binding_by_id(&state.db, binding_id)
        .await?
        .ok_or_else(|| AppError::not_found("workspace binding not found"))?;
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
        (status = 200, body = crate::db::types::WorkspaceBindingResponse),
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
) -> Result<Json<crate::db::types::WorkspaceBindingResponse>, AppError> {
    let (application_id, binding) = resolve_or_create_resource_binding_for_application(
        &state,
        &headers,
        &resource_kind,
        &resource_id,
    )
    .await?;
    acquire_binding_lease(
        &state,
        binding.workspace_binding_id,
        &request.owner,
        request.ttl_seconds,
    )
    .await?;
    crate::db::queries::record_audit(
        &state.db,
        crate::db::audit::AuditLogEntry {
            actor_user_id: None,
            actor_application_id: Some(application_id),
            actor_type: "application",
            action: "workspace.lease.acquire",
            target_type: "workspace_binding",
            target_id: &binding.workspace_binding_id.to_string(),
            workspace_binding_id: Some(binding.workspace_binding_id),
            external_user_id: None,
            payload: json!({ "owner": request.owner, "ttl_seconds": request.ttl_seconds }),
            request_id: None,
            session_id: None,
            duration_ms: None,
            status: Some("success"),
        },
    )
    .await?;
    let binding =
        crate::db::queries::find_workspace_binding_by_id(&state.db, binding.workspace_binding_id)
            .await?
            .ok_or_else(|| AppError::not_found("workspace binding not found"))?;
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
        (status = 200, body = crate::db::types::WorkspaceBindingResponse),
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
) -> Result<Json<crate::db::types::WorkspaceBindingResponse>, AppError> {
    let (application_id, binding) = resolve_or_create_resource_binding_for_application(
        &state,
        &headers,
        &resource_kind,
        &resource_id,
    )
    .await?;
    release_binding_lease(&state, binding.workspace_binding_id, &request.owner).await?;
    crate::db::queries::record_audit(
        &state.db,
        crate::db::audit::AuditLogEntry {
            actor_user_id: None,
            actor_application_id: Some(application_id),
            actor_type: "application",
            action: "workspace.lease.release",
            target_type: "workspace_binding",
            target_id: &binding.workspace_binding_id.to_string(),
            workspace_binding_id: Some(binding.workspace_binding_id),
            external_user_id: None,
            payload: json!({ "owner": request.owner }),
            request_id: None,
            session_id: None,
            duration_ms: None,
            status: Some("success"),
        },
    )
    .await?;
    let binding =
        crate::db::queries::find_workspace_binding_by_id(&state.db, binding.workspace_binding_id)
            .await?
            .ok_or_else(|| AppError::not_found("workspace binding not found"))?;
    Ok(Json(binding))
}
