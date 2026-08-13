use std::path::PathBuf;

use anyhow::Context;
use chrono::Utc;
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
    /// Lease owner carried from the MCP request (e.g. an AI conversation id).
    /// Write access to resource-owned workspaces requires holding the binding's
    /// write lease with this owner.
    pub lease_owner: Option<String>,
}

impl ActorContext {
    /// Verifies the actor may mutate files in its bound workspace.
    ///
    /// Resource-owned workspaces (shared across users) require an active write
    /// lease held by this actor's lease owner. Per-user workspaces are
    /// unaffected.
    pub fn ensure_write_access(&self) -> Result<(), String> {
        let Some(binding) = &self.workspace_binding else {
            return Ok(());
        };
        if binding.resource_kind.is_none() {
            return Ok(());
        }
        let lease_owner = self.lease_owner.as_deref().unwrap_or_default();
        let holds_lease = binding.write_lease_owner.as_deref() == Some(lease_owner)
            && !lease_owner.is_empty()
            && binding
                .write_lease_expires_at
                .is_some_and(|expires| expires > Utc::now());
        if holds_lease {
            return Ok(());
        }
        Err(
            "workspace is read-only: no write lease held by this session. Acquire the write lease (or take it over) before running mutating commands"
                .to_string(),
        )
    }
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
        lease_owner: Option<String>,
    },
}

pub fn actor_from_mcp_context(
    state: &AppState,
    context: &RequestContext<rmcp::RoleServer>,
) -> Result<ActorContext, rmcp::ErrorData> {
    let actor = context
        .extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<McpActor>())
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
        lease_owner: None,
    })
}

pub fn actor_from_application_binding(
    state: &AppState,
    application: ApplicationResponse,
    workspace_binding: WorkspaceBindingResponse,
    external_user_id: String,
    token: ApplicationTokenRecord,
    lease_owner: Option<String>,
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
        lease_owner,
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
            lease_owner,
        } => actor_from_application_binding(
            state,
            *application,
            *workspace_binding,
            external_user_id,
            *token,
            lease_owner,
        ),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use super::{ActorContext, ActorMode};
    use crate::db::types::WorkspaceBindingResponse;

    fn actor_with_binding(
        resource_kind: Option<String>,
        write_lease_owner: Option<String>,
        write_lease_expires_at: Option<chrono::DateTime<Utc>>,
        lease_owner: Option<String>,
    ) -> ActorContext {
        ActorContext {
            mode: ActorMode::ApplicationSubject,
            user: None,
            application: None,
            workspace_binding: Some(WorkspaceBindingResponse {
                workspace_binding_id: 1,
                application_id: 1,
                external_user_id: "__resource__".to_string(),
                workspace_key: "code_project:abc".to_string(),
                external_user_hash: "hash".to_string(),
                workspace_root: "/tmp/ws".to_string(),
                is_active: true,
                last_used_at: Utc::now(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                lifecycle_state: "active".to_string(),
                archived_at: None,
                resource_id: resource_kind.as_ref().map(|_| "abc".to_string()),
                resource_kind,
                write_lease_owner,
                write_lease_acquired_at: None,
                write_lease_expires_at,
            }),
            principal_id: "application:1:user:code_project:abc".to_string(),
            external_user_id: Some("user".to_string()),
            workspace_binding_id: 1,
            workspace_root: std::path::PathBuf::from("/tmp/ws"),
            policy: crate::policy::AccessPolicy::new(
                crate::policy::ALL_SCOPES
                    .iter()
                    .map(|scope| (*scope).to_string()),
                crate::policy::ResourceLimits::unrestricted(false),
            ),
            lease_owner,
        }
    }

    #[test]
    fn user_workspace_never_requires_lease() {
        let actor = actor_with_binding(None, None, None, None);
        assert!(actor.ensure_write_access().is_ok());
    }

    #[test]
    fn resource_workspace_requires_matching_lease() {
        let now = Utc::now();
        let actor = actor_with_binding(
            Some("code_project".to_string()),
            Some("conversation:1".to_string()),
            Some(now + Duration::minutes(10)),
            Some("conversation:1".to_string()),
        );
        assert!(actor.ensure_write_access().is_ok());
    }

    #[test]
    fn resource_workspace_rejects_foreign_or_missing_lease() {
        let now = Utc::now();
        let foreign = actor_with_binding(
            Some("code_project".to_string()),
            Some("conversation:1".to_string()),
            Some(now + Duration::minutes(10)),
            Some("conversation:2".to_string()),
        );
        assert!(foreign.ensure_write_access().is_err());

        let missing = actor_with_binding(Some("code_project".to_string()), None, None, None);
        assert!(missing.ensure_write_access().is_err());

        let expired = actor_with_binding(
            Some("code_project".to_string()),
            Some("conversation:1".to_string()),
            Some(now - Duration::minutes(1)),
            Some("conversation:1".to_string()),
        );
        assert!(expired.ensure_write_access().is_err());
    }
}
