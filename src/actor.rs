use std::path::PathBuf;

use anyhow::Context;
use rmcp::service::RequestContext;

use crate::{
    AppState,
    db::types::{
        ApplicationResponse, ApplicationTokenRecord, McpTokenPolicy, UserRecord,
        WorkspaceBindingResponse,
    },
    error::AppError,
    workspace::{resolve_user_workspace, resolve_workspace_binding_root},
};
use runner_protocol::RunnerOwner;

#[derive(Clone, Debug)]
pub enum ActorMode {
    InternalUser,
    ApplicationSubject,
}

#[derive(Clone, Debug)]
pub struct ActorContext {
    pub mode: ActorMode,
    pub user: Option<UserRecord>,
    pub application: Option<ApplicationResponse>,
    pub workspace_binding: Option<WorkspaceBindingResponse>,
    pub principal_id: String,
    pub external_user_id: Option<String>,
    pub workspace_binding_id: i64,
    pub workspace_root: PathBuf,
    pub policy: crate::policy::AccessPolicy,
}

impl ActorContext {
    pub fn is_admin(&self) -> bool {
        self.user.as_ref().is_some_and(|user| user.is_admin)
    }

    pub fn runner_owner(&self) -> RunnerOwner {
        match self.mode {
            ActorMode::InternalUser => RunnerOwner::InternalUser {
                user_id: self
                    .user
                    .as_ref()
                    .map(|user| user.user_id)
                    .unwrap_or(self.workspace_binding_id),
            },
            ActorMode::ApplicationSubject => RunnerOwner::WorkspaceBinding {
                workspace_binding_id: self.workspace_binding_id,
            },
        }
    }
}

#[derive(Clone, Debug)]
pub enum McpActor {
    InternalUser {
        user: Box<UserRecord>,
        token: McpTokenPolicy,
    },
    ApplicationSubject {
        application: Box<ApplicationResponse>,
        workspace_binding: Box<WorkspaceBindingResponse>,
        external_user_id: String,
        token: Box<ApplicationTokenRecord>,
    },
}

pub fn actor_from_mcp_context(
    state: &AppState,
    context: &RequestContext<rmcp::RoleServer>,
) -> Result<ActorContext, rmcp::ErrorData> {
    let actor = context
        .extensions
        .get::<McpActor>()
        .cloned()
        .ok_or_else(|| rmcp::ErrorData::invalid_request("missing MCP actor context", None))?;
    actor_from_mcp_actor(state, actor)
        .map_err(|error| rmcp::ErrorData::internal_error(format!("{error:?}"), None))
}

pub fn actor_from_web_user(state: &AppState, user: UserRecord) -> Result<ActorContext, AppError> {
    let workspace_root = resolve_user_workspace(&state.config.workspace_root, &user)
        .context("failed to resolve user workspace")
        .map_err(AppError::internal)?;
    Ok(ActorContext {
        mode: ActorMode::InternalUser,
        principal_id: format!("user:{}", user.user_id),
        workspace_binding_id: user.user_id,
        user: Some(user),
        application: None,
        workspace_binding: None,
        external_user_id: None,
        workspace_root,
        policy: crate::policy::AccessPolicy::new(
            state.config.server_scopes.clone(),
            state.config.server_limits.clone(),
        ),
    })
}

pub fn actor_from_application_binding(
    state: &AppState,
    application: ApplicationResponse,
    workspace_binding: WorkspaceBindingResponse,
    external_user_id: String,
    token: ApplicationTokenRecord,
) -> Result<ActorContext, AppError> {
    let workspace_root =
        resolve_workspace_binding_root(&state.config.workspace_root, &workspace_binding)
            .context("failed to resolve application workspace")
            .map_err(AppError::internal)?;
    let policy = crate::policy::AccessPolicy::from_layers(
        &application.default_scopes,
        &token.scopes,
        &state.config.server_scopes,
        application.resource_limits(),
        token.token_limits(),
        state.config.server_limits.clone(),
    );
    Ok(ActorContext {
        mode: ActorMode::ApplicationSubject,
        principal_id: format!(
            "application:{}:{}:{}",
            application.application_id, external_user_id, workspace_binding.workspace_key
        ),
        workspace_binding_id: workspace_binding.workspace_binding_id,
        user: None,
        application: Some(application),
        workspace_binding: Some(workspace_binding),
        external_user_id: Some(external_user_id),
        workspace_root,
        policy,
    })
}

pub(crate) fn actor_from_mcp_actor(
    state: &AppState,
    actor: McpActor,
) -> Result<ActorContext, AppError> {
    match actor {
        McpActor::InternalUser { user, token } => {
            let mut actor = actor_from_web_user(state, *user)?;
            let all_scopes = crate::policy::ALL_SCOPES
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>();
            actor.policy = crate::policy::AccessPolicy::from_layers(
                &all_scopes,
                &token.scopes,
                &state.config.server_scopes,
                crate::policy::ResourceLimits::unrestricted(token.network_enabled),
                crate::policy::ResourceLimits {
                    max_timeout_ms: token
                        .max_timeout_ms
                        .and_then(|value| u64::try_from(value).ok()),
                    max_output_bytes: token
                        .max_output_bytes
                        .and_then(|value| usize::try_from(value).ok()),
                    max_file_bytes: token
                        .max_file_bytes
                        .and_then(|value| usize::try_from(value).ok()),
                    max_sessions: token
                        .max_sessions
                        .and_then(|value| usize::try_from(value).ok()),
                    network_enabled: token.network_enabled,
                },
                state.config.server_limits.clone(),
            );
            Ok(actor)
        }
        McpActor::ApplicationSubject {
            application,
            workspace_binding,
            external_user_id,
            token,
        } => actor_from_application_binding(
            state,
            *application,
            *workspace_binding,
            external_user_id,
            *token,
        ),
    }
}
