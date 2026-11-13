//! Admin API for metadata-first workspace Git upstreams (issue 1180 P27).
//!
//! Responses expose only alias, target metadata, `enabled` and
//! `has_credential`. Audit payloads stay equally narrow; plaintext credentials
//! and ciphertext never enter a response, an audit row or a log line.

pub(crate) mod types;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use axum_extra::extract::cookie::CookieJar;
use serde_json::json;

use crate::{
    AppState,
    api::{
        admin::shared::{record_admin_audit, require_admin},
        validation::{ValidatedJson, ValidatedQuery},
    },
    db::{
        types::{ListWorkspaceGitUpstreamsParams, Page, WorkspaceGitUpstreamResponse},
        workspace_git_upstreams::{
            GitUpstreamError, NewWorkspaceGitUpstream, WorkspaceGitUpstreamUpdate,
            create_workspace_git_upstream as insert_upstream,
            delete_workspace_git_upstream as remove_upstream, set_workspace_git_upstream_enabled,
            update_workspace_git_upstream as modify_upstream,
        },
    },
    error::AppError,
};

use types::{CreateWorkspaceGitUpstreamRequest, UpdateWorkspaceGitUpstreamRequest};

#[utoipa::path(
    get,
    path = "/api/admin/workspace-bindings/{binding_id}/git-upstreams",
    tag = "admin-users",
    params(
        ("binding_id" = i64, Path, description = "Workspace binding identifier"),
        ListWorkspaceGitUpstreamsParams
    ),
    responses(
        (status = 200, body = Page<WorkspaceGitUpstreamResponse>),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse)
    )
)]
pub async fn list_workspace_git_upstreams(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(binding_id): Path<i64>,
    ValidatedQuery(params): ValidatedQuery<ListWorkspaceGitUpstreamsParams>,
) -> Result<Json<Page<WorkspaceGitUpstreamResponse>>, AppError> {
    require_admin(&state, &jar).await?;
    let page = crate::db::queries::list_workspace_git_upstreams(
        &state.db,
        binding_id,
        params.limit,
        params.offset,
    )
    .await
    .map_err(map_error)?;
    Ok(Json(page))
}

#[utoipa::path(
    post,
    path = "/api/admin/workspace-bindings/{binding_id}/git-upstreams",
    tag = "admin-users",
    params(("binding_id" = i64, Path, description = "Workspace binding identifier")),
    request_body = CreateWorkspaceGitUpstreamRequest,
    responses(
        (status = 201, body = WorkspaceGitUpstreamResponse),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse),
        (status = 503, body = crate::error::ErrorResponse)
    )
)]
pub async fn create_workspace_git_upstream(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(binding_id): Path<i64>,
    ValidatedJson(request): ValidatedJson<CreateWorkspaceGitUpstreamRequest>,
) -> Result<(StatusCode, Json<WorkspaceGitUpstreamResponse>), AppError> {
    let admin = require_admin(&state, &jar).await?;
    let upstream = insert_upstream(
        &state.db,
        state.config.credential_protector.as_ref(),
        binding_id,
        NewWorkspaceGitUpstream {
            remote_alias: &request.remote_alias,
            target_url: &request.target_url,
            credential: request
                .credential_username
                .as_deref()
                .zip(request.credential_secret.as_deref()),
        },
    )
    .await
    .map_err(map_error)?;

    record_upstream_audit(&state, &admin, "workspace.git_upstream.create", &upstream).await?;
    Ok((StatusCode::CREATED, Json(upstream)))
}

#[utoipa::path(
    patch,
    path = "/api/admin/workspace-bindings/{binding_id}/git-upstreams/{upstream_id}",
    tag = "admin-users",
    params(
        ("binding_id" = i64, Path, description = "Workspace binding identifier"),
        ("upstream_id" = i64, Path, description = "Git upstream identifier")
    ),
    request_body = UpdateWorkspaceGitUpstreamRequest,
    responses(
        (status = 200, body = WorkspaceGitUpstreamResponse),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse),
        (status = 503, body = crate::error::ErrorResponse)
    )
)]
pub async fn update_workspace_git_upstream(
    State(state): State<AppState>,
    jar: CookieJar,
    Path((binding_id, upstream_id)): Path<(i64, i64)>,
    ValidatedJson(request): ValidatedJson<UpdateWorkspaceGitUpstreamRequest>,
) -> Result<Json<WorkspaceGitUpstreamResponse>, AppError> {
    let admin = require_admin(&state, &jar).await?;
    let upstream = modify_upstream(
        &state.db,
        state.config.credential_protector.as_ref(),
        binding_id,
        upstream_id,
        WorkspaceGitUpstreamUpdate {
            remote_alias: request.remote_alias.as_deref(),
            target_url: request.target_url.as_deref(),
            credential: request
                .credential_username
                .as_deref()
                .zip(request.credential_secret.as_deref()),
            clear_credential: request.clear_credential,
        },
    )
    .await
    .map_err(map_error)?;

    record_upstream_audit(&state, &admin, "workspace.git_upstream.update", &upstream).await?;
    Ok(Json(upstream))
}

#[utoipa::path(
    delete,
    path = "/api/admin/workspace-bindings/{binding_id}/git-upstreams/{upstream_id}",
    tag = "admin-users",
    params(
        ("binding_id" = i64, Path, description = "Workspace binding identifier"),
        ("upstream_id" = i64, Path, description = "Git upstream identifier")
    ),
    responses(
        (status = 204),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse)
    )
)]
pub async fn delete_workspace_git_upstream(
    State(state): State<AppState>,
    jar: CookieJar,
    Path((binding_id, upstream_id)): Path<(i64, i64)>,
) -> Result<StatusCode, AppError> {
    let admin = require_admin(&state, &jar).await?;
    remove_upstream(&state.db, binding_id, upstream_id)
        .await
        .map_err(map_error)?;
    record_admin_audit(
        &state,
        &admin,
        "workspace.git_upstream.delete",
        "workspace_git_upstream",
        upstream_id.to_string(),
        json!({ "binding_id": binding_id }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/admin/workspace-bindings/{binding_id}/git-upstreams/{upstream_id}/enable",
    tag = "admin-users",
    params(
        ("binding_id" = i64, Path, description = "Workspace binding identifier"),
        ("upstream_id" = i64, Path, description = "Git upstream identifier")
    ),
    responses(
        (status = 200, body = WorkspaceGitUpstreamResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse)
    )
)]
pub async fn enable_workspace_git_upstream(
    State(state): State<AppState>,
    jar: CookieJar,
    Path((binding_id, upstream_id)): Path<(i64, i64)>,
) -> Result<Json<WorkspaceGitUpstreamResponse>, AppError> {
    let admin = require_admin(&state, &jar).await?;
    let upstream = set_workspace_git_upstream_enabled(&state.db, binding_id, upstream_id, true)
        .await
        .map_err(map_error)?;
    record_upstream_audit(&state, &admin, "workspace.git_upstream.enable", &upstream).await?;
    Ok(Json(upstream))
}

#[utoipa::path(
    post,
    path = "/api/admin/workspace-bindings/{binding_id}/git-upstreams/{upstream_id}/disable",
    tag = "admin-users",
    params(
        ("binding_id" = i64, Path, description = "Workspace binding identifier"),
        ("upstream_id" = i64, Path, description = "Git upstream identifier")
    ),
    responses(
        (status = 200, body = WorkspaceGitUpstreamResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse)
    )
)]
pub async fn disable_workspace_git_upstream(
    State(state): State<AppState>,
    jar: CookieJar,
    Path((binding_id, upstream_id)): Path<(i64, i64)>,
) -> Result<Json<WorkspaceGitUpstreamResponse>, AppError> {
    let admin = require_admin(&state, &jar).await?;
    let upstream = set_workspace_git_upstream_enabled(&state.db, binding_id, upstream_id, false)
        .await
        .map_err(map_error)?;
    record_upstream_audit(&state, &admin, "workspace.git_upstream.disable", &upstream).await?;
    Ok(Json(upstream))
}

async fn record_upstream_audit(
    state: &AppState,
    admin: &crate::actor::ActorContext,
    action: &'static str,
    upstream: &WorkspaceGitUpstreamResponse,
) -> Result<(), AppError> {
    record_admin_audit(
        state,
        admin,
        action,
        "workspace_git_upstream",
        upstream.upstream_id.to_string(),
        json!({
            "binding_id": upstream.workspace_binding_id,
            "alias": upstream.remote_alias,
            "enabled": upstream.enabled,
            "has_credential": upstream.has_credential,
        }),
    )
    .await
}

fn map_error(error: GitUpstreamError) -> AppError {
    match error {
        GitUpstreamError::BindingNotFound => AppError::not_found("workspace binding not found"),
        GitUpstreamError::NotFound => AppError::not_found("workspace git upstream not found"),
        GitUpstreamError::Forbidden => {
            AppError::forbidden("application authority does not own this workspace binding")
        }
        GitUpstreamError::BindingInactive => AppError::conflict("workspace binding is not active"),
        GitUpstreamError::LeaseConflict => {
            AppError::conflict("workspace write lease is missing, stale or held by another owner")
        }
        GitUpstreamError::DuplicateAlias => {
            AppError::conflict("remote alias already exists for this workspace binding")
        }
        GitUpstreamError::CredentialKeyUnavailable => {
            AppError::service_unavailable("credential encryption key is not configured")
        }
        GitUpstreamError::CredentialUnreadable => {
            AppError::service_unavailable("stored credential could not be decrypted")
        }
        GitUpstreamError::Internal(error) => AppError::internal(error),
    }
}
