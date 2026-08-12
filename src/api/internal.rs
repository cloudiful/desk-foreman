use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use axum_extra::extract::cookie::CookieJar;
use runner_protocol::{RUNNER_JOB_POLL_TIMEOUT_SECS, RunnerJob, RunnerJobResult};
use serde_json::json;

use crate::{
    AppState,
    api::validation::ValidatedJson,
    db::types::{
        ApplicationCapabilitiesResponse, GitWorkspaceSyncRequest, GitWorkspaceSyncResponse,
        RunnerManagerResponse, WorkspaceLeaseReleaseRequest, WorkspaceLeaseRequest,
    },
    error::AppError,
};

use super::admin::workspace_bindings::{acquire_binding_lease, release_binding_lease};

pub(super) fn router() -> axum::Router<AppState> {
    axum::Router::new()
        .route(
            "/api/internal/application/capabilities",
            axum::routing::get(application_capabilities),
        )
        .route(
            "/api/internal/runner-manager/config",
            axum::routing::get(runner_manager_config),
        )
        .route(
            "/api/internal/runner-manager/jobs/next",
            axum::routing::get(next_runner_job),
        )
        .route(
            "/api/internal/runner-manager/jobs/result",
            axum::routing::post(complete_runner_job),
        )
        .route(
            "/api/internal/resource-workspaces/{resource_kind}/{resource_id}/git/sync",
            axum::routing::post(sync_resource_workspace_git),
        )
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

async fn runner_manager_from_token(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<crate::db::types::RunnerManagerRecord, AppError> {
    let authorization = headers
        .get(axum::http::header::AUTHORIZATION)
        .ok_or_else(|| AppError::unauthorized("runner manager token required"))?
        .to_str()
        .map_err(|_| AppError::unauthorized("invalid runner manager token"))?;
    let token = authorization
        .strip_prefix("Bearer ")
        .ok_or_else(|| AppError::unauthorized("invalid runner manager token"))?;
    let manager = crate::db::queries::find_runner_manager_by_token(&state.db, token)
        .await?
        .ok_or_else(|| AppError::not_found("runner manager is not registered"))?;
    if !manager.enabled {
        return Err(AppError::forbidden("runner manager is disabled"));
    }
    crate::db::queries::touch_runner_manager(&state.db, manager.runner_manager_id).await?;
    Ok(manager)
}

#[utoipa::path(
    get,
    path = "/api/internal/runner-manager/config",
    tag = "internal",
    responses(
        (status = 200, body = RunnerManagerResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse)
    )
)]
pub async fn runner_manager_config(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<RunnerManagerResponse>, AppError> {
    let manager = runner_manager_from_token(&state, &headers).await?;
    Ok(Json(RunnerManagerResponse {
        runner_manager_id: manager.runner_manager_id,
        name: manager.name,
        endpoint: manager.endpoint,
        enabled: manager.enabled,
        image: manager.image,
        network_enabled: manager.network_enabled,
        max_output_bytes: manager.max_output_bytes,
        max_timeout_ms: manager.max_timeout_ms,
        max_sessions: manager.max_sessions,
        pids_limit: manager.pids_limit,
        memory_limit: manager.memory_limit,
        cpu_limit: manager.cpu_limit,
        status: manager.status,
        last_seen_at: manager.last_seen_at,
        created_at: manager.created_at,
        updated_at: manager.updated_at,
    }))
}

#[utoipa::path(
    get,
    path = "/api/internal/runner-manager/jobs/next",
    tag = "internal",
    responses((status = 200, body = Option<runner_protocol::RunnerJob>))
)]
pub async fn next_runner_job(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Option<RunnerJob>>, AppError> {
    let manager = runner_manager_from_token(&state, &headers).await?;
    let job = tokio::time::timeout(
        std::time::Duration::from_secs(RUNNER_JOB_POLL_TIMEOUT_SECS),
        state.runner_broker.next_job(manager.runner_manager_id),
    )
    .await
    .unwrap_or(None);
    Ok(Json(job))
}

#[utoipa::path(
    post,
    path = "/api/internal/runner-manager/jobs/result",
    tag = "internal",
    request_body = runner_protocol::RunnerJobResult,
    responses((status = 204))
)]
pub async fn complete_runner_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(result): Json<RunnerJobResult>,
) -> Result<StatusCode, AppError> {
    let manager = runner_manager_from_token(&state, &headers).await?;
    state
        .runner_broker
        .complete_job(manager.runner_manager_id, result)
        .await
        .map_err(AppError::internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/internal/application/capabilities",
    tag = "internal",
    responses(
        (status = 200, body = ApplicationCapabilitiesResponse),
        (status = 401, body = crate::error::ErrorResponse)
    )
)]
pub async fn application_capabilities(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApplicationCapabilitiesResponse>, AppError> {
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
    Ok(Json(ApplicationCapabilitiesResponse {
        application_id: application.application_id,
        application_name: application.name,
        scopes: policy.scopes,
        max_timeout_ms: policy.limits.max_timeout_ms,
        max_output_bytes: policy.limits.max_output_bytes,
        max_file_bytes: policy.limits.max_file_bytes,
        max_sessions: policy.limits.max_sessions,
        network_enabled: policy.limits.network_enabled,
        runner_available,
    }))
}

#[utoipa::path(
    post,
    path = "/api/internal/resource-workspaces/{resource_kind}/{resource_id}/git/sync",
    tag = "internal",
    params(
        ("resource_kind" = String, Path, description = "Resource workspace kind"),
        ("resource_id" = String, Path, description = "Resource workspace identifier")
    ),
    request_body = GitWorkspaceSyncRequest,
    responses(
        (status = 200, body = GitWorkspaceSyncResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse)
    )
)]
pub async fn sync_resource_workspace_git(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((resource_kind, resource_id)): Path<(String, String)>,
    ValidatedJson(request): ValidatedJson<GitWorkspaceSyncRequest>,
) -> Result<Json<GitWorkspaceSyncResponse>, AppError> {
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
    actor.ensure_write_access().map_err(AppError::forbidden)?;
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
    let remote_url = validate_git_remote_url(&request.remote_url)?;
    let needs_initialization = !actor.workspace_root.join(".git").is_dir();
    if needs_initialization
        && actor
            .workspace_root
            .read_dir()
            .map_err(|error| AppError::internal(error.into()))?
            .next()
            .is_some()
    {
        return Err(AppError::conflict(
            "workspace is not empty and is not a git checkout",
        ));
    }
    if !needs_initialization {
        let status = run_git_command(
            &state,
            &actor,
            vec!["status".to_string(), "--porcelain".to_string()],
        )
        .await?;
        if status.exit_code != Some(0) || !status.stdout.trim().is_empty() {
            return Err(AppError::conflict("workspace has uncommitted changes"));
        }
    }
    let output = if needs_initialization {
        let mut args = vec!["clone".to_string(), "--depth".to_string(), "1".to_string()];
        if let Some(branch) = request
            .branch
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            validate_git_branch(branch)?;
            args.extend(["--branch".to_string(), branch.to_string()]);
        }
        args.extend([remote_url, ".".to_string()]);
        run_git_command(&state, &actor, args).await?
    } else {
        let branch = if let Some(branch) = request
            .branch
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            validate_git_branch(branch)?;
            branch.to_string()
        } else {
            let current_branch = run_git_command(
                &state,
                &actor,
                vec!["branch".to_string(), "--show-current".to_string()],
            )
            .await?;
            let current_branch = current_branch.stdout.trim();
            if current_branch.is_empty() {
                return Err(AppError::conflict(
                    "branch is required for a detached git workspace",
                ));
            }
            validate_git_branch(current_branch)?;
            current_branch.to_string()
        };
        let args = vec![
            "pull".to_string(),
            "--ff-only".to_string(),
            remote_url,
            branch,
        ];
        run_git_command(&state, &actor, args).await?
    };
    if output.exit_code != Some(0) {
        return Err(AppError::conflict(format!(
            "git sync failed: {}",
            redact_git_output(&output.output)
        )));
    }
    let head = run_git_command(
        &state,
        &actor,
        vec!["rev-parse".to_string(), "HEAD".to_string()],
    )
    .await?;
    let branch = run_git_command(
        &state,
        &actor,
        vec![
            "rev-parse".to_string(),
            "--abbrev-ref".to_string(),
            "HEAD".to_string(),
        ],
    )
    .await?;
    Ok(Json(GitWorkspaceSyncResponse {
        status: "ready".to_string(),
        cloned: needs_initialization,
        dirty: false,
        branch: Some(branch.stdout.trim().to_string()),
        head_commit: Some(head.stdout.trim().to_string()),
    }))
}

async fn run_git_command(
    state: &AppState,
    actor: &crate::actor::ActorContext,
    args: Vec<String>,
) -> Result<runner_protocol::CommandOutput, AppError> {
    state
        .runner
        .run_command(runner_protocol::RunnerCommandRequest {
            owner: actor.runner_owner(),
            workspace_root: actor.workspace_root.clone(),
            working_dir: actor.workspace_root.clone(),
            program: "git".to_string(),
            args,
            timeout_ms: actor.policy.limits.max_timeout_ms.or(Some(120_000)),
            max_output_bytes: actor.policy.limits.max_output_bytes,
            network_enabled: actor.policy.limits.network_enabled,
        })
        .await
        .map_err(AppError::internal)
}

fn validate_git_remote_url(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 2_048 || value.contains(['\n', '\r']) {
        return Err(AppError::bad_request("invalid git remote url"));
    }
    if !(value.starts_with("https://") || value.starts_with("http://")) {
        return Err(AppError::bad_request(
            "git remote url must use http or https",
        ));
    }
    Ok(value.to_string())
}

fn validate_git_branch(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > 256
        || value.contains(['\n', '\r', ' ', '\t'])
        || value.contains("..")
    {
        return Err(AppError::bad_request("invalid git branch"));
    }
    Ok(())
}

fn redact_git_output(value: &str) -> String {
    value
        .lines()
        .map(|line| line.replace("Authorization:", "Authorization:[redacted]"))
        .collect::<Vec<_>>()
        .join("\n")
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
