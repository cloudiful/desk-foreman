use axum::http::{HeaderMap, header::AUTHORIZATION};

use crate::{
    AppState,
    actor::{ActorContext, McpActor, actor_from_mcp_actor},
    db::{
        queries,
        types::{ApplicationResponse, ApplicationTokenRecord},
    },
    error::AppError,
    workspace::{initialize_workspace_template, resolve_application_workspace},
};

pub(crate) const EXTERNAL_USER_HEADER: &str = "X-DF-External-User-Id";
pub(crate) const WORKSPACE_KEY_HEADER: &str = "X-DF-Workspace-Key";

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
    let external_user_id = header_value(headers, EXTERNAL_USER_HEADER)?
        .ok_or_else(|| AppError::unauthorized("missing X-DF-External-User-Id header"))?;
    let workspace_key =
        header_value(headers, WORKSPACE_KEY_HEADER)?.unwrap_or_else(|| "default".to_string());
    let application_response = application_response(&application);
    let binding = if let Some(binding) = queries::find_workspace_binding_any(
        &state.db,
        application.application_id,
        &external_user_id,
        &workspace_key,
    )
    .await?
    {
        if binding.lifecycle_state != "active" || !binding.is_active {
            return Err(AppError::unauthorized("workspace binding is not active"));
        }
        let _ = queries::touch_workspace_binding(&state.db, binding.workspace_binding_id).await;
        binding
    } else {
        let workspace_root = resolve_application_workspace(
            &state.config.workspace_root,
            &application_response,
            &external_user_id,
            &workspace_key,
        )
        .map_err(AppError::internal)?;
        let binding = queries::create_workspace_binding(
            &state.db,
            application.application_id,
            &external_user_id,
            &workspace_key,
            &workspace_root.to_string_lossy(),
        )
        .await?;
        initialize_workspace_template(
            &state.config.workspace_root,
            &workspace_root,
            application.workspace_template.as_deref(),
        )?;
        binding
    };

    Ok(McpActor::ApplicationSubject {
        application: Box::new(application_response),
        workspace_binding: Box::new(binding),
        external_user_id,
        token: Box::new(application),
    })
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
        approval_mode: application.app_approval_mode.clone(),
        approval_endpoint: application.app_approval_endpoint.clone(),
        approval_model: application.app_approval_model.clone(),
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
