use axum::http::{HeaderMap, header::AUTHORIZATION};

use crate::{
    AppState,
    actor::{ActorContext, McpActor, actor_from_mcp_actor},
    db::{
        queries,
        types::{ApplicationResponse, ApplicationTokenRecord, WorkspaceBindingResponse},
    },
    error::AppError,
    workspace::{
        initialize_workspace_template, parse_resource_workspace_key, resolve_application_workspace,
        resolve_resource_workspace,
    },
};

pub(crate) const EXTERNAL_USER_HEADER: &str = "X-DF-External-User-Id";
pub(crate) const WORKSPACE_KEY_HEADER: &str = "X-DF-Workspace-Key";
pub(crate) const LEASE_OWNER_HEADER: &str = "X-DF-Lease-Owner";
/// Marker external user id for resource-owned shared workspaces.
pub(crate) const RESOURCE_EXTERNAL_USER_MARKER: &str = "__resource__";

pub(crate) async fn current_actor_from_bearer(
    state: &AppState,
    headers: &HeaderMap,
    touch: bool,
) -> Result<Option<ActorContext>, AppError> {
    let Some(actor) = mcp_actor_from_bearer(state, headers, touch).await? else {
        return Ok(None);
    };
    Ok(Some(actor_from_mcp_actor(state, actor)?))
}

pub(crate) async fn mcp_actor_from_bearer(
    state: &AppState,
    headers: &HeaderMap,
    touch: bool,
) -> Result<Option<McpActor>, AppError> {
    let Some(token) = bearer_token(headers)? else {
        return Ok(None);
    };
    let token_hash = crate::auth::hash_bearer_token(token);
    if let Some((user, token_policy)) =
        queries::find_active_mcp_token_hash(&state.db, &token_hash).await?
    {
        if super::runner_isolation::requested(headers)? {
            return Err(AppError::unauthorized(
                "runner isolation requires application authentication",
            ));
        }
        if touch {
            let _ = queries::touch_mcp_token(&state.db, &token_hash).await;
        }
        return Ok(Some(McpActor::InternalUser {
            user: Box::new(user),
            token: token_policy,
        }));
    }
    if let Some(application) =
        queries::find_active_application_token_hash(&state.db, &token_hash).await?
    {
        let actor = application_mcp_actor(state, headers, application).await?;
        if touch {
            let _ = queries::touch_application_token(&state.db, &token_hash).await;
        }
        return Ok(Some(actor));
    }
    Ok(None)
}

pub(crate) async fn application_token_from_bearer(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<ApplicationTokenRecord>, AppError> {
    let Some(token) = bearer_token(headers)? else {
        return Ok(None);
    };
    let token_hash = crate::auth::hash_bearer_token(token);
    let Some(application) =
        queries::find_active_application_token_hash(&state.db, &token_hash).await?
    else {
        return Ok(None);
    };
    let _ = queries::touch_application_token(&state.db, &token_hash).await;
    Ok(Some(application))
}

fn bearer_token(headers: &HeaderMap) -> Result<Option<&str>, AppError> {
    let Some(value) = headers.get(AUTHORIZATION) else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| AppError::unauthorized("invalid authorization header"))?;
    let Some(token) = value.strip_prefix("Bearer ") else {
        return Err(AppError::unauthorized("invalid authorization scheme"));
    };
    Ok(Some(token))
}

async fn application_mcp_actor(
    state: &AppState,
    headers: &HeaderMap,
    application: ApplicationTokenRecord,
) -> Result<McpActor, AppError> {
    let isolated = super::runner_isolation::requested(headers)?;
    let (external_user_id, workspace_key, lease_owner) = if isolated {
        let external = super::runner_isolation::required_claim(headers, EXTERNAL_USER_HEADER)?;
        let workspace = super::runner_isolation::required_claim(headers, WORKSPACE_KEY_HEADER)?;
        let lease = super::runner_isolation::required_claim(headers, LEASE_OWNER_HEADER)?;
        if external != lease
            || parse_resource_workspace_key(&workspace).is_none()
            || !runner_protocol::workspace_principal::is_valid_application_principal_identity(
                application.application_id,
                &external,
                &workspace,
            )
        {
            return Err(AppError::unauthorized(
                "invalid principal workspace identity",
            ));
        }
        (external, workspace, Some(lease))
    } else {
        (
            header_value(headers, EXTERNAL_USER_HEADER)?
                .ok_or_else(|| AppError::unauthorized("missing X-DF-External-User-Id header"))?,
            header_value(headers, WORKSPACE_KEY_HEADER)?.unwrap_or_else(|| "default".to_string()),
            header_value(headers, LEASE_OWNER_HEADER)?,
        )
    };
    require_isolated_resource_workspace(isolated, &workspace_key)?;
    let application_response = application_response(&application);
    let binding =
        if let Some((resource_kind, resource_id)) = parse_resource_workspace_key(&workspace_key) {
            resource_binding(
                state,
                &application_response,
                &resource_kind,
                &resource_id,
                &workspace_key,
            )
            .await?
        } else {
            user_binding(
                state,
                &application_response,
                &external_user_id,
                &workspace_key,
            )
            .await?
        };

    let isolated_runner_owner = if isolated {
        Some(
            runner_protocol::workspace_principal::application_principal_owner(
                application.application_id,
                binding.workspace_binding_id,
                &external_user_id,
                &workspace_key,
            )
            .map_err(|_| AppError::unauthorized("invalid principal workspace identity"))?,
        )
    } else {
        None
    };
    Ok(McpActor::ApplicationSubject {
        isolated_runner_owner,
        application: Box::new(application_response),
        workspace_binding: Box::new(binding),
        external_user_id,
        token: Box::new(application),
        lease_owner,
    })
}

pub(super) fn require_isolated_resource_workspace(
    isolated: bool,
    workspace_key: &str,
) -> Result<(), AppError> {
    if !isolated && parse_resource_workspace_key(workspace_key).is_some() {
        return Err(AppError::forbidden(
            "resource workspace requires an isolated principal",
        ));
    }
    Ok(())
}

async fn user_binding(
    state: &AppState,
    application: &ApplicationResponse,
    external_user_id: &str,
    workspace_key: &str,
) -> Result<WorkspaceBindingResponse, AppError> {
    let existing = queries::find_workspace_binding_any(
        &state.db,
        application.application_id,
        external_user_id,
        workspace_key,
    )
    .await?;
    if let Some(binding) = existing {
        if binding.lifecycle_state != "active" || !binding.is_active {
            return Err(AppError::unauthorized("workspace binding is not active"));
        }
        let _ = queries::touch_workspace_binding(&state.db, binding.workspace_binding_id).await;
        return Ok(binding);
    }
    let workspace_root = resolve_application_workspace(
        &state.config.workspace_root,
        application,
        external_user_id,
        workspace_key,
    )
    .map_err(AppError::internal)?;
    create_binding_with_template(
        state,
        application,
        external_user_id,
        workspace_key,
        &workspace_root,
        None,
        None,
    )
    .await
}

pub(crate) async fn resolve_or_create_resource_binding(
    state: &AppState,
    application: &ApplicationResponse,
    resource_kind: &str,
    resource_id: &str,
    workspace_key: &str,
) -> Result<WorkspaceBindingResponse, AppError> {
    resource_binding(
        state,
        application,
        resource_kind,
        resource_id,
        workspace_key,
    )
    .await
}

async fn resource_binding(
    state: &AppState,
    application: &ApplicationResponse,
    resource_kind: &str,
    resource_id: &str,
    workspace_key: &str,
) -> Result<WorkspaceBindingResponse, AppError> {
    let existing = queries::find_workspace_binding_by_resource(
        &state.db,
        application.application_id,
        resource_kind,
        resource_id,
    )
    .await?;
    if let Some(binding) = existing {
        if binding.lifecycle_state != "active" || !binding.is_active {
            return Err(AppError::unauthorized("workspace binding is not active"));
        }
        let _ = queries::touch_workspace_binding(&state.db, binding.workspace_binding_id).await;
        return Ok(binding);
    }
    let workspace_root = resolve_resource_workspace(
        &state.config.workspace_root,
        application,
        resource_kind,
        resource_id,
    )
    .map_err(AppError::internal)?;
    let binding = create_binding_with_template(
        state,
        application,
        RESOURCE_EXTERNAL_USER_MARKER,
        workspace_key,
        &workspace_root,
        Some(resource_kind),
        Some(resource_id),
    )
    .await?;
    // The create path may fall back to an existing row on a unique conflict;
    // that row could be archived or deactivated, which must not resurrect it.
    if binding.lifecycle_state != "active" || !binding.is_active {
        return Err(AppError::unauthorized("workspace binding is not active"));
    }
    Ok(binding)
}

async fn create_binding_with_template(
    state: &AppState,
    application: &ApplicationResponse,
    external_user_id: &str,
    workspace_key: &str,
    workspace_root: &std::path::Path,
    resource_kind: Option<&str>,
    resource_id: Option<&str>,
) -> Result<WorkspaceBindingResponse, AppError> {
    let binding = queries::create_workspace_binding(
        &state.db,
        application.application_id,
        external_user_id,
        workspace_key,
        &workspace_root.to_string_lossy(),
        resource_kind,
        resource_id,
    )
    .await?;
    initialize_workspace_template(
        &state.config.workspace_root,
        workspace_root,
        application.workspace_template.as_deref(),
    )?;
    Ok(binding)
}

fn application_response(application: &ApplicationTokenRecord) -> ApplicationResponse {
    ApplicationResponse {
        application_id: application.application_id,
        name: application.name.clone(),
        is_active: application.is_active,
        workspace_template: application.workspace_template.clone(),
        default_shell: application.default_shell.clone(),
        created_at: application.created_at,
        updated_at: application.updated_at,
        default_scopes: application.default_scopes.clone(),
        max_timeout_ms: application.app_max_timeout_ms,
        max_output_bytes: application.app_max_output_bytes,
        max_file_bytes: application.app_max_file_bytes,
        max_sessions: application.app_max_sessions,
        network_enabled: application.app_network_enabled,
    }
}

fn header_value(headers: &HeaderMap, name: &str) -> Result<Option<String>, AppError> {
    let Some(value) = headers.get(name) else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| AppError::unauthorized(format!("invalid {name} header")))?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AppError::unauthorized(format!("{name} header is empty")));
    }
    Ok(Some(trimmed.to_string()))
}
