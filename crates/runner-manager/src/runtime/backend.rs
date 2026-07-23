use desk_foreman::runner::RunnerFuture;
use runner_protocol::{CommandOutput, RunnerCommandRequest, RunnerOwner, RunnerShellRequest};

pub trait RunnerBackend: Send + Sync {
    fn reconcile<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<()>>;

    fn prepare_shell_spawn<'a>(
        &'a self,
        request: RunnerShellRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ProcessSpawnTarget>>;

    fn run_command<'a>(
        &'a self,
        request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>>;

    fn reclaim_idle_runners<'a>(
        &'a self,
        active_owners: Vec<RunnerOwner>,
    ) -> RunnerFuture<'a, anyhow::Result<()>>;
}

#[derive(Clone, Debug)]
pub struct ProcessSpawnTarget {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<std::path::PathBuf>,
}
