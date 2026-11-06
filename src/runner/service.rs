use std::time::Duration;

use tokio::sync::OwnedRwLockWriteGuard;

use runner_protocol::{
    CancelSessionRequest, CleanupWorkspaceBindingPrincipalsAck,
    CleanupWorkspaceBindingPrincipalsRequest, CommandOutput, EnsureExecutionEnvironmentAck,
    EnsureExecutionEnvironmentRequest, ExecRequest, GitReceiptReadAck, GitReceiptReadRequest,
    InputRequest, RunnerCommandRequest, RunnerOwner, RunnerSessionStatus, ShellToolOutput,
    StopExecutionEnvironmentAck, StopExecutionEnvironmentRequest, StopWorkspacePrincipalAck,
    StopWorkspacePrincipalRequest,
};

use crate::runner::RunnerFuture;

pub trait RunnerService: Send + Sync {
    fn exec_shell<'a>(
        &'a self,
        request: ExecRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>>;

    fn write_stdin<'a>(
        &'a self,
        request: InputRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>>;

    fn cancel_session<'a>(
        &'a self,
        request: CancelSessionRequest,
    ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>>;

    fn list_sessions<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>>;

    fn run_command<'a>(
        &'a self,
        request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>>;

    fn cleanup_runner_owner<'a>(
        &'a self,
        owner: RunnerOwner,
    ) -> RunnerFuture<'a, anyhow::Result<()>>;

    fn cleanup_workspace_binding_principals<'a>(
        &'a self,
        _request: CleanupWorkspaceBindingPrincipalsRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CleanupWorkspaceBindingPrincipalsAck>> {
        Box::pin(async {
            anyhow::bail!(
                "workspace binding principal cleanup is not supported by this runner service"
            )
        })
    }

    fn stop_workspace_principal<'a>(
        &'a self,
        _request: StopWorkspacePrincipalRequest,
    ) -> RunnerFuture<'a, anyhow::Result<StopWorkspacePrincipalAck>> {
        Box::pin(async move {
            anyhow::bail!("workspace principal stop is not supported by this runner service")
        })
    }

    /// Agent-neutral ensure. Default fails closed so existing
    /// `PullRunnerService` and test fakes keep compiling without behavior
    /// change; the manager backend overrides with the trusted-profile path.
    fn ensure_execution_environment<'a>(
        &'a self,
        _request: EnsureExecutionEnvironmentRequest,
    ) -> RunnerFuture<'a, anyhow::Result<EnsureExecutionEnvironmentAck>> {
        Box::pin(async move {
            anyhow::bail!("execution environments are not supported by this runner service")
        })
    }

    /// Agent-neutral stop with inspect-confirmed acknowledgment. Default
    /// fails closed; the manager backend overrides.
    fn stop_execution_environment<'a>(
        &'a self,
        _request: StopExecutionEnvironmentRequest,
    ) -> RunnerFuture<'a, anyhow::Result<StopExecutionEnvironmentAck>> {
        Box::pin(async move {
            anyhow::bail!("execution environments are not supported by this runner service")
        })
    }

    /// Acquire the workspace-binding install fence while no session exists.
    /// Default refuses so a runner without the same-key fence never installs.
    fn acquire_git_install_fence<'a>(
        &'a self,
        _workspace_binding_id: i64,
        _remaining: Duration,
    ) -> RunnerFuture<'a, Option<OwnedRwLockWriteGuard<()>>> {
        Box::pin(async { None })
    }

    /// Read one manager-side publication receipt without any write.
    ///
    /// Default refuses so a runner that cannot prove a receipt never reports
    /// publication evidence. The manager backend overrides this method.
    fn read_git_receipt<'a>(
        &'a self,
        _request: GitReceiptReadRequest,
    ) -> RunnerFuture<'a, anyhow::Result<GitReceiptReadAck>> {
        Box::pin(async {
            anyhow::bail!("git receipt read is not supported by this runner service")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runner_protocol::{
        CancelSessionRequest, CleanupWorkspaceBindingPrincipalsRequest,
        EnsureExecutionEnvironmentRequest, ExecRequest, InputRequest, RunnerCommandRequest,
        RunnerOwner,
    };

    struct DefaultService;

    impl RunnerService for DefaultService {
        fn exec_shell<'a>(
            &'a self,
            _: ExecRequest,
        ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
            Box::pin(async { anyhow::bail!("unsupported") })
        }
        fn write_stdin<'a>(
            &'a self,
            _: InputRequest,
        ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
            Box::pin(async { anyhow::bail!("unsupported") })
        }
        fn cancel_session<'a>(
            &'a self,
            _: CancelSessionRequest,
        ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>> {
            Box::pin(async { anyhow::bail!("unsupported") })
        }
        fn list_sessions<'a>(
            &'a self,
        ) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>> {
            Box::pin(async { anyhow::bail!("unsupported") })
        }
        fn run_command<'a>(
            &'a self,
            _: RunnerCommandRequest,
        ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
            Box::pin(async { anyhow::bail!("unsupported") })
        }
        fn cleanup_runner_owner<'a>(
            &'a self,
            _: RunnerOwner,
        ) -> RunnerFuture<'a, anyhow::Result<()>> {
            Box::pin(async { anyhow::bail!("unsupported") })
        }
        fn cleanup_workspace_binding_principals<'a>(
            &'a self,
            _: CleanupWorkspaceBindingPrincipalsRequest,
        ) -> RunnerFuture<'a, anyhow::Result<CleanupWorkspaceBindingPrincipalsAck>> {
            Box::pin(async { anyhow::bail!("unsupported") })
        }
        fn ensure_execution_environment<'a>(
            &'a self,
            _: EnsureExecutionEnvironmentRequest,
        ) -> RunnerFuture<'a, anyhow::Result<EnsureExecutionEnvironmentAck>> {
            Box::pin(async { anyhow::bail!("unsupported") })
        }
    }

    #[tokio::test]
    async fn default_install_fence_denies() {
        assert!(
            DefaultService
                .acquire_git_install_fence(1, Duration::from_secs(1))
                .await
                .is_none()
        );
    }
}
