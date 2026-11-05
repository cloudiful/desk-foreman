//! Internal application-facing endpoints: capability introspection and
//! resource workspace git sync.

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};

use crate::{AppState, error::AppError};

use super::shared::valid_resource_path_component;

#[utoipa::path(
    get,
    path = "/api/internal/application/capabilities",
    tag = "internal",
    security(("application_bearer" = [])),
    responses(
        (status = 200, body = crate::db::types::ApplicationCapabilitiesResponse),
        (status = 401, body = crate::error::ErrorResponse)
    )
)]
pub async fn application_capabilities(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<crate::db::types::ApplicationCapabilitiesResponse>, AppError> {
    let Some(token) =
        crate::auth::identity::application_token_from_bearer(&state, &headers).await?
    else {
        return Err(AppError::unauthorized("application bearer token required"));
    };
    let application = crate::db::queries::find_application_by_id(&state.db, token.application_id)
        .await?
        .ok_or_else(|| AppError::not_found("application not found"))?;
    let policy = crate::policy::AccessPolicy::from_layers(
        &application.default_scopes,
        &token.scopes,
        &state.config.server_scopes,
        application.resource_limits(),
        token.token_limits(),
        state.config.server_limits.clone(),
    );
    let runner_available = state.runner.list_sessions().await.is_ok();
    Ok(Json(crate::db::types::ApplicationCapabilitiesResponse {
        application_id: application.application_id,
        application_name: application.name,
        scopes: policy.scopes,
        max_timeout_ms: policy.limits.max_timeout_ms,
        max_output_bytes: policy.limits.max_output_bytes,
        max_file_bytes: policy.limits.max_file_bytes,
        max_sessions: policy.limits.max_sessions,
        network_enabled: policy.limits.network_enabled,
        runner_available,
        execution_environments_ready: false,
    }))
}

#[utoipa::path(
    post,
    path = "/api/internal/resource-workspaces/{resource_kind}/{resource_id}/git/sync",
    tag = "internal",
    security(("application_bearer" = [])),
    params(
        ("resource_kind" = String, Path, description = "Resource workspace kind"),
        ("resource_id" = String, Path, description = "Resource workspace identifier")
    ),
    request_body = crate::db::types::GitWorkspaceSyncRequest,
    responses(
        (status = 200, body = crate::db::types::GitWorkspaceSyncResponse),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse),
        (status = 503, body = crate::error::ErrorResponse)
    )
)]
pub async fn sync_resource_workspace_git(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((resource_kind, resource_id)): Path<(String, String)>,
    crate::api::validation::ValidatedJson(request): crate::api::validation::ValidatedJson<
        crate::db::types::GitWorkspaceSyncRequest,
    >,
) -> Result<Json<crate::db::types::GitWorkspaceSyncResponse>, AppError> {
    if resource_kind != "code_project" || !valid_resource_path_component(&resource_id) {
        return Err(AppError::bad_request("invalid resource workspace key"));
    }
    let expected_workspace_key = format!("{resource_kind}:{resource_id}");
    let workspace_key = headers
        .get(crate::auth::identity::WORKSPACE_KEY_HEADER)
        .ok_or_else(|| AppError::unauthorized("missing X-DF-Workspace-Key header"))?
        .to_str()
        .map_err(|_| AppError::unauthorized("invalid X-DF-Workspace-Key header"))?
        .trim();
    if workspace_key != expected_workspace_key {
        return Err(AppError::forbidden(
            "workspace key does not match resource path",
        ));
    }
    let Some(actor) =
        crate::auth::identity::current_actor_from_bearer(&state, &headers, false).await?
    else {
        return Err(AppError::unauthorized("application bearer token required"));
    };
    let Some(binding) = actor.workspace_binding.as_ref() else {
        return Err(AppError::unauthorized("application bearer token required"));
    };
    if binding.resource_kind.as_deref() != Some(resource_kind.as_str())
        || binding.resource_id.as_deref() != Some(resource_id.as_str())
        || binding.workspace_key != expected_workspace_key
    {
        return Err(AppError::forbidden(
            "workspace binding does not match resource path",
        ));
    }
    if !actor.policy.allows(crate::policy::WORKSPACE_SHELL) {
        return Err(AppError::forbidden(
            "workspace.shell scope is required for git sync",
        ));
    }
    if !actor.policy.limits.network_enabled {
        return Err(AppError::forbidden(
            "network access is required for git sync",
        ));
    }

    super::git_sync_job::run(&state, &actor, &request, &resource_kind, &resource_id)
        .await
        .map(Json)
}

#[cfg(test)]
#[path = "application_git_sync_tests.rs"]
mod tests;
