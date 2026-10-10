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
            tracing::error!(job_id, kind = %job.kind, error = %error, "runner job failed");
            RunnerJobResult {
                job_id,
                ok: false,
                result: None,
                error: Some(error.to_string()),
            }
        }
    }
}
