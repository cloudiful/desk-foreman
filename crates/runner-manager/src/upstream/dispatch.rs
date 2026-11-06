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
use runner_protocol::{GitReceiptReadRequest, RunnerJob, RunnerJobResult};

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
    if job.kind == "git_operation" {
        return super::git_operation::execute_job(runner, config, job).await;
    }
    if job.kind == "read_git_receipt" {
        return read_git_receipt_job(runner, job).await;
    }
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

const RECEIPT_UNAVAILABLE: &str = "git receipt read unavailable";
const RECEIPT_UNCONFIRMED: &str = "git receipt read is unconfirmed";

/// Read-only receipt lookup over the authenticated manager pull channel.
///
/// The payload is parsed and validated before the runner call, and the ack is
/// validated against the same request. Failures never include paths, URLs,
/// credentials, or raw Git errors.
pub(super) async fn read_git_receipt_job(
    runner: &dyn RunnerService,
    job: RunnerJob,
) -> RunnerJobResult {
    let job_id = job.job_id.clone();
    let Ok(request) = serde_json::from_value::<GitReceiptReadRequest>(job.payload) else {
        return receipt_failure(job_id, RECEIPT_UNAVAILABLE);
    };
    if request.validate().is_err() {
        return receipt_failure(job_id, RECEIPT_UNAVAILABLE);
    }
    let Ok(ack) = runner.read_git_receipt(request.clone()).await else {
        return receipt_failure(job_id, RECEIPT_UNAVAILABLE);
    };
    if ack.validate_matches(&request).is_err() {
        return receipt_failure(job_id, RECEIPT_UNCONFIRMED);
    }
    let Ok(value) = serde_json::to_value(ack) else {
        return receipt_failure(job_id, RECEIPT_UNAVAILABLE);
    };
    RunnerJobResult {
        job_id,
        ok: true,
        result: Some(value),
        error: None,
    }
}

fn receipt_failure(job_id: String, message: &str) -> RunnerJobResult {
    RunnerJobResult {
        job_id,
        ok: false,
        result: None,
        error: Some(message.to_owned()),
    }
}
