use runner_protocol::{
    CancelSessionRequest, CommandOutput, ExecRequest, InputRequest, RunnerCommandRequest,
    RunnerOwner, RunnerSessionStatus, ShellToolOutput,
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
}
