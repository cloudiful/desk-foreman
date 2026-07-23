use std::{path::Path, process::Stdio, sync::Arc};

use anyhow::Context;
use desk_foreman::runner::RunnerFuture;
use runner_protocol::{CommandOutput, RunnerCommandRequest, RunnerOwner, RunnerShellRequest};
use tokio::process::Command;

use super::{ProcessSpawnTarget, RunnerBackend, shell_spawn::append_shell_args};

pub struct DirectRunnerBackend;

impl DirectRunnerBackend {
    pub fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

impl RunnerBackend for DirectRunnerBackend {
    fn reconcile<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn prepare_shell_spawn<'a>(
        &'a self,
        request: RunnerShellRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ProcessSpawnTarget>> {
        Box::pin(async move {
            Ok(ProcessSpawnTarget {
                program: request.shell,
                args: append_shell_args(request.login, &request.command),
                env: vec![(
                    "WORKSPACE_ROOT".to_string(),
                    request.workspace_root.to_string_lossy().to_string(),
                )],
                cwd: Some(request.working_dir),
            })
        })
    }

    fn run_command<'a>(
        &'a self,
        request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
        Box::pin(async move {
            let output = command_output(
                &request.program,
                &request.args,
                &request.working_dir,
                &request.workspace_root,
                request.timeout_ms,
                request.max_output_bytes,
            )
            .await?;
            Ok(output)
        })
    }

    fn reclaim_idle_runners<'a>(
        &'a self,
        _active_owners: Vec<RunnerOwner>,
    ) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

async fn command_output(
    program: &str,
    args: &[String],
    working_dir: &Path,
    workspace_root: &Path,
    timeout_ms: Option<u64>,
    max_output_bytes: Option<usize>,
) -> anyhow::Result<CommandOutput> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(working_dir)
        .env("WORKSPACE_ROOT", workspace_root.as_os_str())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    super::bounded_command_output(command, timeout_ms, max_output_bytes.unwrap_or(256 * 1024))
        .await
        .with_context(|| format!("failed to start {program}"))
}
