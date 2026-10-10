use std::sync::Arc;

use desk_foreman::runner::RunnerFuture;
use runner_protocol::{
    CommandOutput, EnsureExecutionEnvironmentAck, EnsureExecutionEnvironmentRequest,
    NativeTaskPreparedResult, PrepareNativeTaskRequest, RunnerCommandRequest, RunnerOwner,
    RunnerShellRequest, SeedBundle, StopExecutionEnvironmentAck, StopExecutionEnvironmentRequest,
};

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

    fn cleanup_runner_owner<'a>(
        &'a self,
        owner: RunnerOwner,
    ) -> RunnerFuture<'a, anyhow::Result<()>>;

    /// Agent-neutral environment ensure. Default fails closed; the Docker
    /// backend overrides with the trusted-profile implementation.
    fn ensure_execution_environment<'a>(
        &'a self,
        _request: EnsureExecutionEnvironmentRequest,
    ) -> RunnerFuture<'a, anyhow::Result<EnsureExecutionEnvironmentAck>> {
        Box::pin(async move {
            anyhow::bail!("execution environments are not supported by this runner backend")
        })
    }

    /// Agent-neutral environment stop with inspect-confirmed acknowledgment.
    /// Default fails closed; the Docker backend overrides.
    fn stop_execution_environment<'a>(
        &'a self,
        _request: StopExecutionEnvironmentRequest,
    ) -> RunnerFuture<'a, anyhow::Result<StopExecutionEnvironmentAck>> {
        Box::pin(async move {
            anyhow::bail!("execution environments are not supported by this runner backend")
        })
    }

    /// Materialize one task's private checkout from its validated seed.
    ///
    /// Default fails closed with a fixed unsupported failure, so a backend
    /// that cannot prepare (a direct backend, or a manager with no trusted
    /// root) never pretends to have materialized anything. The prepared
    /// snapshot arrives already fetched and validated through the shared
    /// protocol rules; only the filesystem work happens here.
    fn prepare_native_task<'a>(
        &'a self,
        _request: PrepareNativeTaskRequest,
        _seed: SeedBundle,
    ) -> RunnerFuture<'a, anyhow::Result<NativeTaskPreparedResult>> {
        Box::pin(async move {
            anyhow::bail!("native task preparation is not configured on this runner backend")
        })
    }

    fn begin_shell_operation(&self, _owner: &RunnerOwner) {}

    fn end_shell_operation(&self, _owner: &RunnerOwner) {}

    fn touch_activity(&self, _owner: &RunnerOwner) {}
}

pub(crate) struct RunnerOperationLease {
    backend: Arc<dyn RunnerBackend>,
    owner: RunnerOwner,
}

impl RunnerOperationLease {
    pub(crate) fn new(backend: Arc<dyn RunnerBackend>, owner: RunnerOwner) -> Self {
        backend.begin_shell_operation(&owner);
        Self { backend, owner }
    }
}

impl Drop for RunnerOperationLease {
    fn drop(&mut self) {
        self.backend.end_shell_operation(&self.owner);
    }
}

pub(crate) struct BorrowedRunnerOperationLease<'a> {
    backend: &'a dyn RunnerBackend,
    owner: RunnerOwner,
}

impl<'a> BorrowedRunnerOperationLease<'a> {
    pub(crate) fn new(backend: &'a dyn RunnerBackend, owner: RunnerOwner) -> Self {
        backend.begin_shell_operation(&owner);
        Self { backend, owner }
    }
}

impl Drop for BorrowedRunnerOperationLease<'_> {
    fn drop(&mut self) {
        self.backend.end_shell_operation(&self.owner);
    }
}

#[derive(Clone, Debug)]
pub struct ProcessSpawnTarget {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<std::path::PathBuf>,
}
