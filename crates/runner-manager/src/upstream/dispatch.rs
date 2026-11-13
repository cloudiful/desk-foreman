//! Which executor runs one polled job (issue 753 P2br).
//!
//! One decision lives here: a durable `prepare_native_task` job goes to the
//! native-task dispatcher, and every other kind keeps its existing in-process
//! execution. The outcome is normalized into the single `RunnerJobResult` the
//! poll loop submits, so no caller has to know which path ran.
//!
//! This is organization only: the routing, the token/identity lookup and the
//! error mapping are unchanged.

use std::sync::Arc;

use desk_foreman::runner::RunnerService;
use reqwest::Client;
use runner_protocol::{RunnerJob, RunnerJobResult};

#[path = "native_execution.rs"]
mod native_execution;
#[path = "native_task_export.rs"]
mod native_task_export;
#[cfg(test)]
pub(crate) use native_execution::ProfiledDockerBackend;

use super::execute::execute_job_inner;
use super::native_task;
use crate::config::SharedRunnerManagerConfig;
use crate::runtime::RunnerBackend;
use crate::runtime::session_gate::SessionGate;

/// Execute one polled job and shape its result.
pub(super) async fn execute_job(
    runner: &dyn RunnerService,
    backend: &dyn RunnerBackend,
    client: &Client,
    base_url: &str,
    config: &SharedRunnerManagerConfig,
    gate: &Arc<SessionGate>,
    job: RunnerJob,
) -> RunnerJobResult {
    let job_id = job.job_id.clone();
    let outcome = if job.kind == "prepare_native_task" {
        let (token, manager_id) = {
            let guard = config.read().await;
            (guard.auth_token.clone(), guard.manager_id.clone())
        };
        native_task::execute_job(client, base_url, &token, &manager_id, backend, &job).await
    } else if job.kind == "export_native_task" {
        let (token, manager_id) = {
            let guard = config.read().await;
            (guard.auth_token.clone(), guard.manager_id.clone())
        };
        native_task_export::execute_job(client, base_url, &token, &manager_id, backend, &job).await
    } else if matches!(job.kind.as_str(), "start_native_task" | "stop_native_task") {
        native_execution::execute_job(backend, &job).await
    } else {
        execute_job_inner(runner, config, gate, &job.kind, job.payload).await
    };
    match outcome {
        Ok(value) => RunnerJobResult {
            job_id,
            ok: true,
            result: Some(value),
            error: None,
        },
        Err(error) => {
            let message = job_failure_message(&job.kind, &error);
            tracing::error!(job_id, kind = %job.kind, error = %message, "runner job failed");
            RunnerJobResult {
                job_id,
                ok: false,
                result: None,
                error: Some(message),
            }
        }
    }
}

/// `write_stdin` keeps its full source chain in log and result; other kinds keep top-level context.
fn job_failure_message(kind: &str, error: &anyhow::Error) -> String {
    if kind == "write_stdin" {
        format!("{error:#}")
    } else {
        error.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::job_failure_message;

    fn broken_pipe_error() -> anyhow::Error {
        anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "write side closed",
        ))
        .context("failed to write to session input (session_id 7)")
    }

    #[test]
    fn write_stdin_failure_keeps_full_chain() {
        let message = job_failure_message("write_stdin", &broken_pipe_error());
        assert_eq!(
            message,
            "failed to write to session input (session_id 7): write side closed"
        );
    }

    #[test]
    fn other_job_kinds_keep_top_level_context() {
        let message = job_failure_message("exec_shell", &broken_pipe_error());
        assert_eq!(message, "failed to write to session input (session_id 7)");
    }
}
