use std::sync::Arc;

use desk_foreman::runner::{RunnerFuture, RunnerService};
use runner_protocol::{
    CancelSessionRequest, ExecRequest, InputRequest, RunnerCommandRequest, RunnerOwner,
    RunnerSessionStatus, ShellToolOutput,
};

use super::{RunnerBackend, shell_manager::ShellManager};
use crate::config::SharedRunnerManagerConfig;

pub struct LocalRunnerService {
    backend: Arc<dyn RunnerBackend>,
    shell: Arc<ShellManager>,
}

impl LocalRunnerService {
    pub fn new(backend: Arc<dyn RunnerBackend>, config: SharedRunnerManagerConfig) -> Arc<Self> {
        let shell = Arc::new(ShellManager::new(Arc::clone(&backend), config));
        Arc::new(Self { backend, shell })
    }

    pub async fn reconcile(&self) -> anyhow::Result<()> {
        self.backend.reconcile().await
    }
}

impl RunnerService for LocalRunnerService {
    fn exec_shell<'a>(
        &'a self,
        request: ExecRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async move { self.shell.exec(request).await })
    }

    fn write_stdin<'a>(
        &'a self,
        request: InputRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async move { self.shell.write_stdin(request).await })
    }

    fn cancel_session<'a>(
        &'a self,
        request: CancelSessionRequest,
    ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>> {
        Box::pin(async move { self.shell.cancel_session(request).await })
    }

    fn list_sessions<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>> {
        Box::pin(async move { self.shell.list_sessions().await })
    }

    fn run_command<'a>(
        &'a self,
        request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<runner_protocol::CommandOutput>> {
        Box::pin(async move { self.backend.run_command(request).await })
    }

    fn cleanup_runner_owner<'a>(
        &'a self,
        owner: RunnerOwner,
    ) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
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
}
