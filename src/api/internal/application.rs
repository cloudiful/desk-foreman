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
    request_body = crate::db::types::GitWorkspaceSyncRequest,
    responses(
        (status = 200, body = crate::db::types::GitWorkspaceSyncResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse)
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
    Ok(Json(crate::db::types::GitWorkspaceSyncResponse {
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
        .map_err(|error| {
            tracing::error!(error = %error, "workspace runner command failed");
            AppError::service_unavailable("workspace runner is unavailable")
        })
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
