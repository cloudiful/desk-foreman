use std::sync::Arc;
use std::time::Duration;

use desk_foreman::runner::{RunnerFuture, RunnerService};
use runner_protocol::{
    CancelSessionRequest, CleanupWorkspaceBindingPrincipalsAck,
    CleanupWorkspaceBindingPrincipalsRequest, EnsureExecutionEnvironmentAck,
    EnsureExecutionEnvironmentRequest, ExecRequest, GitReceiptReadAck, GitReceiptReadRequest,
    InputRequest, RunnerCommandRequest, RunnerOwner, RunnerSessionStatus, ShellToolOutput,
    StopExecutionEnvironmentAck, StopExecutionEnvironmentRequest, StopWorkspacePrincipalAck,
    StopWorkspacePrincipalRequest,
};
use tokio::sync::OwnedRwLockWriteGuard;

use super::{RunnerBackend, shell_manager::ShellManager};
use crate::config::SharedRunnerManagerConfig;

#[path = "workspace_principal_stop.rs"]
mod workspace_principal_stop;
use workspace_principal_stop::OwnerFences;

#[cfg(test)]
#[path = "workspace_principal_stop_tests.rs"]
mod workspace_principal_stop_tests;

pub struct LocalRunnerService {
    backend: Arc<dyn RunnerBackend>,
    shell: Arc<ShellManager>,
    config: SharedRunnerManagerConfig,
    fences: OwnerFences,
    binding_fences: OwnerFences,
}

impl LocalRunnerService {
    pub fn new(backend: Arc<dyn RunnerBackend>, config: SharedRunnerManagerConfig) -> Arc<Self> {
        let shell = Arc::new(ShellManager::new(Arc::clone(&backend), Arc::clone(&config)));
        Arc::new(Self {
            backend,
            shell,
            config,
            fences: OwnerFences::default(),
            binding_fences: OwnerFences::default(),
        })
    }

    async fn binding_read(
        &self,
        owner: &RunnerOwner,
    ) -> Option<tokio::sync::OwnedRwLockReadGuard<()>> {
        let workspace_binding_id = binding_id(owner)?;
        Some(
            self.binding_fences
                .for_owner(&RunnerOwner::WorkspaceBinding {
                    workspace_binding_id,
                })
                .read_owned()
                .await,
        )
    }

    async fn binding_has_live_sessions(&self, workspace_binding_id: i64) -> bool {
        match self.shell.snapshot_sessions().await {
            Ok(sessions) => sessions
                .iter()
                .any(|session| binding_id(&session.owner) == Some(workspace_binding_id)),
            Err(_) => true,
        }
    }

    async fn read_git_receipt_inner(
        &self,
        request: &GitReceiptReadRequest,
    ) -> anyhow::Result<GitReceiptReadAck> {
        use super::git_authority::clone_plan::GitCloneConfig;
        use super::git_authority::docker_runner::{DockerCliExecutor, GitPlanRunner};
        use super::git_authority::receipt::{GitReceiptError, read_receipt};
        use super::git_authority::update::inspect_published;

        let config = self.config.read().await.clone();
        let Some(binding) = request.workspace_binding_id() else {
            anyhow::bail!("git receipt read is unconfirmed");
        };
        let root = std::path::Path::new(&request.workspace_root);
        let manager_root = match std::fs::canonicalize(&config.workspace_root) {
            Ok(root) => root,
            Err(_) => anyhow::bail!("git receipt read is unavailable"),
        };
        let canonical_root = match std::fs::canonicalize(root) {
            Ok(root) => root,
            Err(_) => return Ok(unconfirmed_ack(request, binding)),
        };
        if canonical_root != root || !canonical_root.starts_with(&manager_root) {
            return Ok(unconfirmed_ack(request, binding));
        }
        let Ok(clone_config) = GitCloneConfig::from_runner(&config) else {
            return Ok(unconfirmed_ack(request, binding));
        };
        let receipt = match read_receipt(&clone_config.private_root, request) {
            Ok(receipt) => receipt,
            Err(GitReceiptError::Unavailable) => return Ok(missing_ack(request, binding)),
            Err(_) => return Ok(unconfirmed_ack(request, binding)),
        };
        let Ok(executor) =
            DockerCliExecutor::new(config.docker_cli.clone(), config.docker_host.clone())
        else {
            return Ok(unconfirmed_ack(request, binding));
        };
        let executor = GitPlanRunner::new(executor);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        let remaining = (request.deadline_unix_ms - now_ms).clamp(1, 30_000) as u64;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(remaining);
        let inspected = inspect_published(
            &request.workspace_root,
            &request.canonical_branch,
            &receipt.head,
            &clone_config,
            &executor,
            request.operation_id,
            deadline,
        )
        .await;
        let Some((head, branch, clean)) = inspected else {
            return Ok(unconfirmed_ack(request, binding));
        };
        let ack = receipt
            .read_ack()
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        if receipt.is_confirmed()
            && clean
            && head == receipt.head
            && branch == request.canonical_branch
            && ack.job_id == request.job_id
            && ack.operation_id == request.operation_id
            && ack.workspace_binding_id == binding
        {
            Ok(ack)
        } else {
            Ok(unconfirmed_ack(request, binding))
        }
    }

    pub async fn reconcile(&self) -> anyhow::Result<()> {
        self.backend.reconcile().await
    }
}

fn binding_id(owner: &RunnerOwner) -> Option<i64> {
    match owner {
        RunnerOwner::ApplicationPrincipal {
            workspace_binding_id,
            ..
        }
        | RunnerOwner::WorkspaceBinding {
            workspace_binding_id,
        } => Some(*workspace_binding_id),
        _ => None,
    }
}

fn base_ack(request: &GitReceiptReadRequest, binding: i64) -> GitReceiptReadAck {
    GitReceiptReadAck {
        job_id: request.job_id,
        operation_id: request.operation_id,
        workspace_binding_id: binding,
        state: runner_protocol::GitReceiptState::Missing,
        phase: None,
        base: request.expected_base.clone(),
        head: None,
        published: false,
        cleanup_confirmed: false,
    }
}

fn missing_ack(request: &GitReceiptReadRequest, binding: i64) -> GitReceiptReadAck {
    base_ack(request, binding)
}

fn unconfirmed_ack(request: &GitReceiptReadRequest, binding: i64) -> GitReceiptReadAck {
    GitReceiptReadAck {
        state: runner_protocol::GitReceiptState::Unconfirmed,
        ..base_ack(request, binding)
    }
}

impl RunnerService for LocalRunnerService {
    fn exec_shell<'a>(
        &'a self,
        request: ExecRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async move {
            let _binding_guard = self.binding_read(&request.owner).await;
            let fence = self.fences.for_owner(&request.owner);
            let _guard = fence.read().await;
            self.shell.exec(request).await
        })
    }

    fn write_stdin<'a>(
        &'a self,
        request: InputRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async move {
            let _binding_guard = self.binding_read(&request.owner).await;
            let fence = self.fences.for_owner(&request.owner);
            let _guard = fence.read().await;
            self.shell.write_stdin(request).await
        })
    }

    fn cancel_session<'a>(
        &'a self,
        request: CancelSessionRequest,
    ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>> {
        Box::pin(async move {
            let _binding_guard = self.binding_read(&request.owner).await;
            let fence = self.fences.for_owner(&request.owner);
            let _guard = fence.read().await;
            self.shell.cancel_session(request).await
        })
    }

    fn list_sessions<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>> {
        Box::pin(async move { self.shell.list_sessions().await })
    }

    fn run_command<'a>(
        &'a self,
        request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<runner_protocol::CommandOutput>> {
        Box::pin(async move {
            let _binding_guard = self.binding_read(&request.owner).await;
            let fence = self.fences.for_owner(&request.owner);
            let _guard = fence.read().await;
            if matches!(request.owner, RunnerOwner::ApplicationPrincipal { .. })
                && !self.backend.supports_workspace_principal_stop()
            {
                anyhow::bail!("principal execution is unsupported by this backend");
            }
            self.backend.run_command(request).await
        })
    }

    fn cleanup_runner_owner<'a>(
        &'a self,
        owner: RunnerOwner,
    ) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let _binding_guard = self.binding_read(&owner).await;
            let fence = self.fences.for_owner(&owner);
            let _guard = fence.write().await;
            let sessions = self.shell.list_sessions().await?;
            for session in sessions
                .into_iter()
                .filter(|session| session.owner == owner)
            {
                if let Err(error) = self
                    .shell
                    .cancel_session(CancelSessionRequest {
                        owner: owner.clone(),
                        session_key: session.session_key,
                        session_id: session.session_id,
                    })
                    .await
                {
                    tracing::warn!(
                        owner = %owner.stable_key(),
                        session_id = session.session_id,
                        error = %error,
                        "failed to cancel session during owner cleanup"
                    );
                }
            }
            self.backend.cleanup_runner_owner(owner).await
        })
    }

    fn cleanup_workspace_binding_principals<'a>(
        &'a self,
        request: CleanupWorkspaceBindingPrincipalsRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CleanupWorkspaceBindingPrincipalsAck>> {
        Box::pin(async move {
            use runner_protocol::StopWorkspacePrincipalState as State;
            let ack = |state, detail: Option<String>| CleanupWorkspaceBindingPrincipalsAck {
                state,
                owners: Vec::new(),
                detail,
            };
            if request.workspace_binding_id <= 0 {
                return Ok(ack(State::Conflict, Some("invalid binding".into())));
            }
            if !self.backend.supports_workspace_binding_principal_cleanup() {
                return Ok(ack(State::Unsupported, None));
            }
            let binding = RunnerOwner::WorkspaceBinding {
                workspace_binding_id: request.workspace_binding_id,
            };
            let fence = self.binding_fences.for_owner(&binding);
            let _guard =
                match tokio::time::timeout(std::time::Duration::from_secs(30), fence.write()).await
                {
                    Ok(guard) => guard,
                    Err(_) => return Ok(ack(State::Busy, None)),
                };
            let sessions = match self.shell.snapshot_sessions().await {
                Ok(s) => s,
                Err(e) => return Ok(ack(State::Failed, Some(e.to_string()))),
            };
            for session in sessions.into_iter().filter(|s| matches!(&s.owner, RunnerOwner::ApplicationPrincipal { workspace_binding_id, .. } if *workspace_binding_id == request.workspace_binding_id)) {
                if let Err(e) = self.shell.cancel_session(CancelSessionRequest { owner: session.owner, session_key: session.session_key, session_id: session.session_id }).await {
                    return Ok(ack(State::Failed, Some(e.to_string())));
                }
            }
            let result = self
                .backend
                .cleanup_workspace_binding_principals(request.clone())
                .await?;
            if result.state != State::Stopped {
                return Ok(result);
            }
            match self.shell.snapshot_sessions().await {
                Ok(sessions) if !sessions.iter().any(|s| matches!(&s.owner, RunnerOwner::ApplicationPrincipal { workspace_binding_id, .. } if *workspace_binding_id == request.workspace_binding_id)) => Ok(result),
                Ok(_) => Ok(ack(State::Failed, Some("sessions remain".into()))),
                Err(e) => Ok(ack(State::Failed, Some(e.to_string()))),
            }
        })
    }

    fn stop_workspace_principal<'a>(
        &'a self,
        request: StopWorkspacePrincipalRequest,
    ) -> RunnerFuture<'a, anyhow::Result<StopWorkspacePrincipalAck>> {
        Box::pin(async move {
            use runner_protocol::StopWorkspacePrincipalState as State;
            let ack = |state, detail: Option<String>| StopWorkspacePrincipalAck { state, detail };
            if !request.is_valid() {
                return Ok(ack(State::Conflict, Some("invalid principal owner".into())));
            }
            if !self.backend.supports_workspace_principal_stop() {
                return Ok(ack(State::Unsupported, None));
            }
            let _binding_guard = self.binding_read(&request.owner).await;
            let fence = self.fences.for_owner(&request.owner);
            let _guard =
                match tokio::time::timeout(std::time::Duration::from_secs(30), fence.write()).await
                {
                    Ok(guard) => guard,
                    Err(_) => return Ok(ack(State::Busy, None)),
                };
            let sessions = match self.shell.snapshot_sessions().await {
                Ok(sessions) => sessions,
                Err(error) => return Ok(ack(State::Failed, Some(error.to_string()))),
            };
            if sessions.iter().any(|s| {
                s.owner == request.owner
                    && s.session_key.as_deref() != Some(request.principal_id.as_str())
            }) {
                return Ok(ack(State::Conflict, None));
            }
            for session in sessions.into_iter().filter(|s| s.owner == request.owner) {
                if let Err(error) = self
                    .shell
                    .cancel_session(CancelSessionRequest {
                        owner: request.owner.clone(),
                        session_key: session.session_key,
                        session_id: session.session_id,
                    })
                    .await
                {
                    return Ok(ack(State::Failed, Some(error.to_string())));
                }
            }
            let result = self
                .backend
                .stop_workspace_principal(request.clone())
                .await?;
            if result.state != State::Stopped {
                return Ok(result);
            }
            match self.shell.snapshot_sessions().await {
                Ok(sessions) if !sessions.iter().any(|s| s.owner == request.owner) => Ok(result),
                Ok(_) => Ok(ack(State::Failed, Some("sessions remain".into()))),
                Err(error) => Ok(ack(State::Failed, Some(error.to_string()))),
            }
        })
    }

    fn ensure_execution_environment<'a>(
        &'a self,
        request: EnsureExecutionEnvironmentRequest,
    ) -> RunnerFuture<'a, anyhow::Result<EnsureExecutionEnvironmentAck>> {
        Box::pin(async move { self.backend.ensure_execution_environment(request).await })
    }

    fn stop_execution_environment<'a>(
        &'a self,
        request: StopExecutionEnvironmentRequest,
    ) -> RunnerFuture<'a, anyhow::Result<StopExecutionEnvironmentAck>> {
        Box::pin(async move { self.backend.stop_execution_environment(request).await })
    }

    fn acquire_git_install_fence<'a>(
        &'a self,
        workspace_binding_id: i64,
        remaining: Duration,
    ) -> RunnerFuture<'a, Option<OwnedRwLockWriteGuard<()>>> {
        Box::pin(async move {
            if workspace_binding_id <= 0 || remaining.is_zero() {
                return None;
            }
            let fence = self
                .binding_fences
                .for_owner(&RunnerOwner::WorkspaceBinding {
                    workspace_binding_id,
                });
            let budget = remaining.min(Duration::from_secs(30));
            let guard = match tokio::time::timeout(budget, fence.write_owned()).await {
                Ok(guard) => guard,
                Err(_) => return None,
            };
            if self.binding_has_live_sessions(workspace_binding_id).await {
                return None;
            }
            Some(guard)
        })
    }

    fn read_git_receipt<'a>(
        &'a self,
        request: GitReceiptReadRequest,
    ) -> RunnerFuture<'a, anyhow::Result<GitReceiptReadAck>> {
        Box::pin(async move {
            if request.validate().is_err() {
                anyhow::bail!("git receipt read is unconfirmed");
            }
            let manager_id = self
                .config
                .read()
                .await
                .manager_id
                .parse::<i64>()
                .ok()
                .filter(|id| *id == request.assigned_manager_id)
                .ok_or_else(|| anyhow::anyhow!("git receipt read is unavailable"))?;
            let _ = manager_id;
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0);
            if request.deadline_unix_ms <= now_ms {
                anyhow::bail!("git receipt read is unconfirmed");
            }
            let workspace_binding_id = request
                .workspace_binding_id()
                .ok_or_else(|| anyhow::anyhow!("git receipt read is unconfirmed"))?;
            let fence = self
                .binding_fences
                .for_owner(&RunnerOwner::WorkspaceBinding {
                    workspace_binding_id,
                });
            let remaining =
                Duration::from_millis((request.deadline_unix_ms - now_ms).min(30_000) as u64);
            let _guard = match tokio::time::timeout(remaining, fence.read_owned()).await {
                Ok(guard) => guard,
                Err(_) => anyhow::bail!("git receipt read is unconfirmed"),
            };
            if self.binding_has_live_sessions(workspace_binding_id).await {
                anyhow::bail!("git receipt read is unconfirmed");
            }
            self.read_git_receipt_inner(&request).await
        })
    }
}
