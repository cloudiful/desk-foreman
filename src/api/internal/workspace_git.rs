//! Internal application Git gateway discovery and capability minting
//! (issue 1180 P28).
//!
//! Responses never contain an upstream secret or target URL. The capability is
//! short-lived and scoped to one workspace binding; it is not an application
//! bearer and cannot be used as one.

pub(crate) mod types;

#[cfg(test)]
mod tests;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use chrono::Utc;

use crate::{
    AppState,
    actor::check_write_lease,
    api::{
        internal::shared::resolve_binding_for_application,
        validation::{ValidatedJson, ValidatedQuery},
    },
    db::{
        queries,
        types::{ListWorkspaceGitUpstreamsParams, Page, WorkspaceGitUpstreamResponse},
        workspace_git_upstreams::{
            GitUpstreamError, MutationAuthority, NewWorkspaceGitUpstream,
            WorkspaceGitUpstreamUpdate,
            create_workspace_git_upstream_for_application as insert_application_upstream,
            delete_workspace_git_upstream_for_application as remove_application_upstream,
            update_workspace_git_upstream_for_application as modify_application_upstream,
        },
    },
    error::AppError,
    git_gateway::{
        capability::{
            CapabilityAlias, CapabilityClaims, mint, principal_id as capability_principal_id,
        },
        capability_expiry, gateway_enabled, public_base_url, remote_url, signing_key,
    },
    policy::{WORKSPACE_PATCH, WORKSPACE_SHELL},
};

use types::{
    CreateInternalWorkspaceGitUpstreamRequest, DeleteInternalWorkspaceGitUpstreamQuery,
    UpdateInternalWorkspaceGitUpstreamRequest, WorkspaceGitCapabilityRequest,
    WorkspaceGitCapabilityResponse, WorkspaceGitGatewayResponse, WorkspaceGitUpstreamView,
};

#[utoipa::path(
    get,
    path = "/api/internal/workspace-bindings/{binding_id}/git-gateway",
    tag = "internal-git",
    params(("binding_id" = i64, Path, description = "Workspace binding identifier")),
    security(("application_bearer" = [])),
    responses(
        (status = 200, body = WorkspaceGitGatewayResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse),
        (status = 503, body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn git_gateway_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(binding_id): Path<i64>,
) -> Result<Json<WorkspaceGitGatewayResponse>, AppError> {
    if !gateway_enabled(&state.config) {
        return Err(AppError::service_unavailable("git gateway is disabled"));
    }
    let application_id = resolve_binding_for_application(&state, &headers, binding_id).await?;
    let binding = active_binding(&state, binding_id).await?;
    if binding.application_id != application_id {
        return Err(AppError::forbidden(
            "workspace binding belongs to another application",
        ));
    }
    let base = public_base_url(&state.config)
        .ok_or_else(|| AppError::service_unavailable("git gateway is disabled"))?;
    Ok(Json(WorkspaceGitGatewayResponse {
        gateway_base_url: base.to_string(),
        upstreams: views(&state, binding_id).await?,
    }))
}

#[utoipa::path(
    post,
    path = "/api/internal/workspace-bindings/{binding_id}/git-capability",
    tag = "internal-git",
    params(("binding_id" = i64, Path, description = "Workspace binding identifier")),
    request_body = WorkspaceGitCapabilityRequest,
    security(("application_bearer" = [])),
    responses(
        (status = 200, body = WorkspaceGitCapabilityResponse),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse),
        (status = 503, body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn issue_git_capability(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(binding_id): Path<i64>,
    ValidatedJson(request): ValidatedJson<WorkspaceGitCapabilityRequest>,
) -> Result<Json<WorkspaceGitCapabilityResponse>, AppError> {
    if !gateway_enabled(&state.config) {
        return Err(AppError::service_unavailable("git gateway is disabled"));
    }
    if !request.is_valid() {
        return Err(AppError::bad_request("invalid Git capability request"));
    }
    let application_id = resolve_binding_for_application(&state, &headers, binding_id).await?;
    let binding = active_binding(&state, binding_id).await?;
    if binding.application_id != application_id {
        return Err(AppError::forbidden(
            "workspace binding belongs to another application",
        ));
    }
    if request.external_user_id != binding.external_user_id {
        return Err(AppError::forbidden(
            "Git capability subject does not match the binding",
        ));
    }
    let lease_owner = request.lease_owner.clone().unwrap_or_default();
    let write = check_write_lease(&binding, Some(&lease_owner)).is_ok()
        && application_has_write_scope(&state, &headers).await?;
    if binding.resource_kind.is_some() && !write {
        return Err(AppError::forbidden(
            "Git write capability requires the current workspace lease and write scope",
        ));
    }

    let aliases = queries::list_gateway_aliases(&state.db, binding_id)
        .await
        .map_err(AppError::internal)?;
    let now = Utc::now();
    let expires_at = capability_expiry(&binding, now);
    let claims = CapabilityClaims {
        v: 1,
        app_id: application_id,
        binding_id,
        external_user_id: binding.external_user_id.clone(),
        principal_id: request.principal_id.clone().unwrap_or_else(|| {
            capability_principal_id(
                application_id,
                &binding.external_user_id,
                &binding.workspace_key,
            )
        }),
        lease_owner,
        read: true,
        write,
        aliases: aliases
            .iter()
            .map(|alias| CapabilityAlias {
                remote_alias: alias.remote_alias.clone(),
                upstream_id: alias.upstream_id,
                generation: alias.capability_generation,
            })
            .collect(),
        iat: now.timestamp(),
        exp: expires_at.timestamp(),
    };
    let Some(key) = signing_key(&state.config) else {
        return Err(AppError::service_unavailable("git gateway is disabled"));
    };
    let capability = mint(&key, &claims).map_err(AppError::internal)?;
    let base = public_base_url(&state.config)
        .ok_or_else(|| AppError::service_unavailable("git gateway is disabled"))?;
    Ok(Json(WorkspaceGitCapabilityResponse {
        capability,
        expires_at,
        gateway_base_url: base.to_string(),
        upstreams: views(&state, binding_id).await?,
    }))
}

#[utoipa::path(
    get,
    path = "/api/internal/workspace-bindings/{binding_id}/git-upstreams",
    tag = "internal-git",
    params(
        ("binding_id" = i64, Path, description = "Workspace binding identifier"),
        ListWorkspaceGitUpstreamsParams
    ),
    security(("application_bearer" = [])),
    responses(
        (status = 200, body = Page<WorkspaceGitUpstreamResponse>),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn list_application_git_upstreams(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(binding_id): Path<i64>,
    ValidatedQuery(params): ValidatedQuery<ListWorkspaceGitUpstreamsParams>,
) -> Result<Json<Page<WorkspaceGitUpstreamResponse>>, AppError> {
    read_application_binding(&state, &headers, binding_id).await?;
    let page =
        queries::list_workspace_git_upstreams(&state.db, binding_id, params.limit, params.offset)
            .await
            .map_err(map_upstream_error)?;
    Ok(Json(page))
}

#[utoipa::path(
    post,
    path = "/api/internal/workspace-bindings/{binding_id}/git-upstreams",
    tag = "internal-git",
    params(("binding_id" = i64, Path, description = "Workspace binding identifier")),
    request_body = CreateInternalWorkspaceGitUpstreamRequest,
    security(("application_bearer" = [])),
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
pub(crate) async fn create_application_git_upstream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(binding_id): Path<i64>,
    ValidatedJson(request): ValidatedJson<CreateInternalWorkspaceGitUpstreamRequest>,
) -> Result<(StatusCode, Json<WorkspaceGitUpstreamResponse>), AppError> {
    let (application_id, subject) = write_authority(&state, &headers, binding_id).await?;
    let authority = MutationAuthority::ApplicationSubject {
        application_id,
        external_user_id: &subject,
        lease_owner: &request.lease_owner,
    };
    let upstream = insert_application_upstream(
        &state.db,
        state.config.upstream_credential_key.as_ref(),
        &authority,
        binding_id,
        NewWorkspaceGitUpstream {
            remote_alias: &request.upstream.remote_alias,
            target_url: &request.upstream.target_url,
            credential: request
                .upstream
                .credential_username
                .as_deref()
                .zip(request.upstream.credential_secret.as_deref()),
        },
    )
    .await
    .map_err(map_upstream_error)?;
    Ok((StatusCode::CREATED, Json(upstream)))
}

#[utoipa::path(
    patch,
    path = "/api/internal/workspace-bindings/{binding_id}/git-upstreams/{upstream_id}",
    tag = "internal-git",
    params(
        ("binding_id" = i64, Path, description = "Workspace binding identifier"),
        ("upstream_id" = i64, Path, description = "Git upstream identifier")
    ),
    request_body = UpdateInternalWorkspaceGitUpstreamRequest,
    security(("application_bearer" = [])),
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
pub(crate) async fn update_application_git_upstream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((binding_id, upstream_id)): Path<(i64, i64)>,
    ValidatedJson(request): ValidatedJson<UpdateInternalWorkspaceGitUpstreamRequest>,
) -> Result<Json<WorkspaceGitUpstreamResponse>, AppError> {
    let (application_id, subject) = write_authority(&state, &headers, binding_id).await?;
    let authority = MutationAuthority::ApplicationSubject {
        application_id,
        external_user_id: &subject,
        lease_owner: &request.lease_owner,
    };
    let upstream = modify_application_upstream(
        &state.db,
        state.config.upstream_credential_key.as_ref(),
        &authority,
        binding_id,
        upstream_id,
        WorkspaceGitUpstreamUpdate {
            remote_alias: request.upstream.remote_alias.as_deref(),
            target_url: request.upstream.target_url.as_deref(),
            credential: request
                .upstream
                .credential_username
                .as_deref()
                .zip(request.upstream.credential_secret.as_deref()),
            clear_credential: request.upstream.clear_credential,
        },
    )
    .await
    .map_err(map_upstream_error)?;
    Ok(Json(upstream))
}

#[utoipa::path(
    delete,
    path = "/api/internal/workspace-bindings/{binding_id}/git-upstreams/{upstream_id}",
    tag = "internal-git",
    params(
        ("binding_id" = i64, Path, description = "Workspace binding identifier"),
        ("upstream_id" = i64, Path, description = "Git upstream identifier"),
        DeleteInternalWorkspaceGitUpstreamQuery
    ),
    security(("application_bearer" = [])),
    responses(
        (status = 204),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse)
    )
)]
pub(crate) async fn delete_application_git_upstream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((binding_id, upstream_id)): Path<(i64, i64)>,
    ValidatedQuery(query): ValidatedQuery<DeleteInternalWorkspaceGitUpstreamQuery>,
) -> Result<StatusCode, AppError> {
    let (application_id, subject) = write_authority(&state, &headers, binding_id).await?;
    let authority = MutationAuthority::ApplicationSubject {
        application_id,
        external_user_id: &subject,
        lease_owner: &query.lease_owner,
    };
    remove_application_upstream(&state.db, &authority, binding_id, upstream_id)
        .await
        .map_err(map_upstream_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn read_application_binding(
    state: &AppState,
    headers: &HeaderMap,
    binding_id: i64,
) -> Result<crate::db::types::WorkspaceBindingResponse, AppError> {
    let application_id = match resolve_binding_for_application(state, headers, binding_id).await {
        Ok(application_id) => application_id,
        Err(AppError::Forbidden(_)) => {
            return Err(AppError::not_found("workspace binding not found"));
        }
        Err(error) => return Err(error),
    };
    let binding = queries::find_workspace_binding_by_id(&state.db, binding_id)
        .await?
        .ok_or_else(|| AppError::not_found("workspace binding not found"))?;
    if binding.application_id != application_id {
        return Err(AppError::not_found("workspace binding not found"));
    }
    if binding.external_user_id != request_subject(headers)? {
        return Err(AppError::forbidden("workspace binding subject mismatch"));
    }
    Ok(binding)
}

async fn write_authority(
    state: &AppState,
    headers: &HeaderMap,
    binding_id: i64,
) -> Result<(i64, String), AppError> {
    let application_id = resolve_binding_for_application(state, headers, binding_id).await?;
    if !application_has_write_scope(state, headers).await? {
        return Err(AppError::forbidden(
            "Git upstream writes require workspace write scope",
        ));
    }
    Ok((application_id, request_subject(headers)?))
}

fn request_subject(headers: &HeaderMap) -> Result<String, AppError> {
    let value = headers
        .get(crate::auth::identity::EXTERNAL_USER_HEADER)
        .ok_or_else(|| AppError::unauthorized("missing X-DF-External-User-Id header"))?
        .to_str()
        .map_err(|_| AppError::unauthorized("invalid X-DF-External-User-Id header"))?
        .trim();
    if value.is_empty() {
        return Err(AppError::unauthorized(
            "X-DF-External-User-Id header is empty",
        ));
    }
    Ok(value.to_string())
}

fn map_upstream_error(error: GitUpstreamError) -> AppError {
    match error {
        GitUpstreamError::BindingNotFound | GitUpstreamError::NotFound => {
            AppError::not_found("workspace git upstream not found")
        }
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
        GitUpstreamError::CredentialKeyMissing => {
            AppError::service_unavailable("upstream credential key is not configured")
        }
        GitUpstreamError::Internal(error) => AppError::internal(error),
    }
}

async fn application_has_write_scope(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<bool, AppError> {
    let Some(actor) = crate::auth::identity::mcp_actor_from_bearer(state, headers, false).await?
    else {
        return Err(AppError::unauthorized(
            "bearer token required for internal API",
        ));
    };
    let crate::actor::McpActor::ApplicationSubject { token, .. } = actor else {
        return Err(AppError::unauthorized(
            "application bearer token required for internal API",
        ));
    };
    Ok(token
        .scopes
        .iter()
        .any(|scope| scope == WORKSPACE_PATCH || scope == WORKSPACE_SHELL))
}

async fn active_binding(
    state: &AppState,
    binding_id: i64,
) -> Result<crate::db::types::WorkspaceBindingResponse, AppError> {
    let binding = queries::find_workspace_binding_by_id(&state.db, binding_id)
        .await?
        .ok_or_else(|| AppError::not_found("workspace binding not found"))?;
    if !binding.is_active || binding.lifecycle_state != "active" || binding.archived_at.is_some() {
        return Err(AppError::conflict("workspace binding is not active"));
    }
    Ok(binding)
}

async fn views(
    state: &AppState,
    binding_id: i64,
) -> Result<Vec<WorkspaceGitUpstreamView>, AppError> {
    let aliases = queries::list_gateway_aliases(&state.db, binding_id).await?;
    let mut views = Vec::with_capacity(aliases.len());
    for alias in aliases {
        let Some(url) = remote_url(&state.config, binding_id, &alias.remote_alias) else {
            return Err(AppError::service_unavailable("git gateway is disabled"));
        };
        views.push(WorkspaceGitUpstreamView {
            remote_alias: alias.remote_alias,
            enabled: true,
            capability_generation: alias.capability_generation,
            remote_url: url,
        });
    }
    Ok(views)
}
