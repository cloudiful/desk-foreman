mod backend;
mod direct;
mod docker;
mod local_service;
pub(crate) mod session_gate;
mod shell_manager;
mod shell_session;
mod shell_spawn;

pub use backend::{ProcessSpawnTarget, RunnerBackend};
pub use direct::DirectRunnerBackend;
pub use docker::DockerRunnerBackend;
pub use local_service::LocalRunnerService;

use std::time::Instant;

use runner_protocol::CommandOutput;
use tokio::{
    io::AsyncReadExt,
    process::Command,
    time::{Duration, timeout},
};

pub async fn bounded_command_output(
    mut command: Command,
    timeout_ms: Option<u64>,
    max_output_bytes: usize,
) -> anyhow::Result<CommandOutput> {
    let started = Instant::now();
    let mut child = command.spawn()?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("stdout not available"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("stderr not available"))?;
    let collect = async {
        let stdout_read = read_limited(&mut stdout, max_output_bytes);
        let stderr_read = read_limited(&mut stderr, max_output_bytes);
        let (stdout, stderr, status) = tokio::join!(stdout_read, stderr_read, child.wait());
        Ok::<_, anyhow::Error>((stdout?, stderr?, status?))
    };
    let (stdout, stderr, status, timed_out) = if let Some(ms) = timeout_ms {
        match timeout(Duration::from_millis(ms), collect).await {
            Ok(Ok((stdout, stderr, status))) => (stdout, stderr, Some(status), false),
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                (Vec::new(), Vec::new(), None, true)
            }
        }
    } else {
        let (stdout, stderr, status) = collect.await?;
        (stdout, stderr, Some(status), false)
    };
    let stdout_truncated = stdout.len() > max_output_bytes;
    let stderr_truncated = stderr.len() > max_output_bytes;
    let stdout_bytes = stdout.len();
    let stderr_bytes = stderr.len();
    let stdout = String::from_utf8_lossy(&stdout[..stdout.len().min(max_output_bytes)]).to_string();
    let stderr = String::from_utf8_lossy(&stderr[..stderr.len().min(max_output_bytes)]).to_string();
    Ok(CommandOutput {
        wall_time_seconds: started.elapsed().as_secs_f64(),
        output: format!("{stdout}{stderr}"),
        stdout,
        stderr,
        exit_code: status.and_then(|status| status.code()),
        truncated: stdout_truncated || stderr_truncated,
        timed_out,
        stdout_bytes,
        stderr_bytes,
    })
}

async fn read_limited<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(limit.saturating_add(1).min(8192));
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await?;
    Ok(bytes)
}
