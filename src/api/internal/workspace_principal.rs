use crate::{AppState, error::AppError};
use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::HeaderMap,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct PrincipalWorkspaceCapabilitiesResponse {
    pub protocol_version: u32,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StopWorkspacePrincipalRequest {
    pub expected_lease_owner: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StopWorkspacePrincipalResponse {
    pub principal_stopped: bool,
    pub lease_released: bool,
    pub workspace_binding_id: i64,
}

#[utoipa::path(
    get, path = "/api/internal/principal-workspace-capabilities", tag = "internal",
    security(("application_bearer" = [])),
    responses((status = 200, body = PrincipalWorkspaceCapabilitiesResponse), (status = 401, body = crate::error::ErrorResponse))
)]
pub async fn principal_workspace_capabilities(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<PrincipalWorkspaceCapabilitiesResponse>, AppError> {
    let token = crate::auth::identity::application_token_from_bearer(&state, &headers)
        .await?
        .ok_or_else(|| AppError::unauthorized("application bearer token required"))?;
    crate::db::queries::find_application_by_id(&state.db, token.application_id)
        .await?
        .ok_or_else(|| AppError::unauthorized("application bearer token required"))?;
    Ok(Json(PrincipalWorkspaceCapabilitiesResponse {
        protocol_version: 1,
    }))
}

#[utoipa::path(
    post, path = "/api/internal/resource-workspaces/{resource_kind}/{resource_id}/workspace-sessions/stop", tag = "internal",
    security(("application_bearer" = [])),
    params(("resource_kind" = String, Path), ("resource_id" = String, Path),
        ("X-DF-Runner-Isolation" = String, Header), ("X-DF-External-User-Id" = String, Header),
        ("X-DF-Workspace-Key" = String, Header), ("X-DF-Lease-Owner" = String, Header)),
    request_body = StopWorkspacePrincipalRequest,
    responses((status = 200, body = StopWorkspacePrincipalResponse), (status = 400, body = crate::error::ErrorResponse),
      (status = 401, body = crate::error::ErrorResponse), (status = 403, body = crate::error::ErrorResponse),
      (status = 404, body = crate::error::ErrorResponse), (status = 409, body = crate::error::ErrorResponse),
      (status = 503, body = crate::error::ErrorResponse))
)]
pub async fn stop_workspace_principal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((kind, id)): Path<(String, String)>,
    payload: Result<Json<StopWorkspacePrincipalRequest>, JsonRejection>,
) -> Result<Json<StopWorkspacePrincipalResponse>, AppError> {
    let token = crate::auth::identity::application_token_from_bearer(&state, &headers)
        .await?
        .ok_or_else(|| AppError::unauthorized("application bearer token required"))?;
    let application = crate::db::queries::find_application_by_id(&state.db, token.application_id)
        .await?
        .ok_or_else(|| AppError::unauthorized("application bearer token required"))?;
    let policy = crate::policy::AccessPolicy::from_layers(
        &application.default_scopes,
        &token.scopes,
        &state.config.server_scopes,
        application.resource_limits(),
        token.token_limits(),
        state.config.server_limits.clone(),
    );
    if !policy.allows(crate::policy::WORKSPACE_SHELL) {
        return Err(AppError::forbidden("workspace.shell scope required"));
    }
    let Json(request) = payload.map_err(|_| AppError::bad_request("invalid stop request"))?;
    let invalid = || AppError::bad_request("invalid principal workspace claims");
    if !super::shared::valid_resource_path_component(&kind)
        || !super::shared::valid_resource_path_component(&id)
        || !crate::auth::runner_isolation::requested(&headers).map_err(|_| invalid())?
    {
        return Err(invalid());
    }
    let claim =
        |name| crate::auth::runner_isolation::required_claim(&headers, name).map_err(|_| invalid());
    let external = claim(crate::auth::identity::EXTERNAL_USER_HEADER)?;
    let workspace = claim(crate::auth::identity::WORKSPACE_KEY_HEADER)?;
    let lease = claim(crate::auth::identity::LEASE_OWNER_HEADER)?;
    if external != lease
        || lease != request.expected_lease_owner
        || workspace != format!("{kind}:{id}")
        || crate::workspace::parse_resource_workspace_key(&workspace).is_none()
    {
        return Err(invalid());
    }
    let binding = crate::db::queries::find_workspace_binding_by_resource(
        &state.db,
        application.application_id,
        &kind,
        &id,
    )
    .await?
    .ok_or_else(|| AppError::not_found("workspace binding not found"))?;
    let owner = runner_protocol::workspace_principal::application_principal_owner(
        application.application_id,
        binding.workspace_binding_id,
        &external,
        &workspace,
    )
    .map_err(|_| invalid())?;
    let principal_id = format!(
        "application:{}:{}:{}",
        application.application_id, external, workspace
    );
    let (mut tx, fresh) = crate::db::workspace_bindings::lock_workspace_binding_for_write(
        &state.db,
        binding.workspace_binding_id,
    )
    .await?;
    let fresh = fresh
        .ok_or_else(|| AppError::conflict("workspace unavailable or native task active; retry"))?;
    if fresh.application_id != application.application_id
        || fresh.resource_kind.as_deref() != Some(kind.as_str())
        || fresh.resource_id.as_deref() != Some(id.as_str())
        || fresh.workspace_key != workspace
        || !fresh.is_active
        || fresh.lifecycle_state != "active"
    {
        return Err(AppError::not_found("workspace binding not found"));
    }
    let stop = runner_protocol::StopWorkspacePrincipalRequest {
        owner,
        principal_id,
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(25),
        crate::runner::principal_stop::stop_owner(&state.db, state.runner.as_ref(), stop),
    )
    .await;
    if !matches!(
        result,
        Ok(Ok(runner_protocol::StopWorkspacePrincipalAck {
            state: runner_protocol::StopWorkspacePrincipalState::Stopped,
            ..
        }))
    ) {
        return Err(AppError::service_unavailable(
            "principal stop unconfirmed; retry",
        ));
    }
    let lease_released = if fresh.write_lease_owner.as_deref() == Some(lease.as_str()) {
        crate::db::workspace_bindings::release_workspace_write_lease_in_transaction(
            &mut tx,
            fresh.workspace_binding_id,
            &lease,
        )
        .await?
        .ok_or_else(|| AppError::conflict("lease changed; retry"))?;
        true
    } else {
        false
    };
    tx.commit().await?;
    Ok(Json(StopWorkspacePrincipalResponse {
        principal_stopped: true,
        lease_released,
        workspace_binding_id: fresh.workspace_binding_id,
    }))
}

#[cfg(test)]
#[path = "workspace_principal_tests.rs"]
mod tests;
