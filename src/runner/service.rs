use runner_protocol::{
    CancelSessionRequest, CleanupWorkspaceBindingPrincipalsAck,
    CleanupWorkspaceBindingPrincipalsRequest, CommandOutput, EnsureExecutionEnvironmentAck,
    EnsureExecutionEnvironmentRequest, ExecRequest, InputRequest, RunnerCommandRequest,
    RunnerOwner, RunnerSessionStatus, ShellToolOutput, StopExecutionEnvironmentAck,
    StopExecutionEnvironmentRequest, StopWorkspacePrincipalAck, StopWorkspacePrincipalRequest,
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
}
