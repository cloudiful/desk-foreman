//! Durable assigned-manager environment-job transport (issue 742 P2).
//!
//! Thin wiring between the internal runner-manager routes and the durable
//! environment-job dispatch helpers. The reserved opaque wire identity
//! `envjob:<UUID>` keeps durable jobs disjoint from the legacy in-memory
//! broker, so a result either routes here or to the unchanged legacy path,
//! never both. The authenticated manager identity is established by the
//! caller before either route runs.

use axum::http::StatusCode;
use runner_protocol::{RunnerJob, RunnerJobResult};

use crate::{
    AppState,
    db::execution_environment_jobs::{
        DurableJobCompletion, claim_next_assigned_manager_environment_job,
        complete_assigned_manager_environment_job, encode_environment_job_wire_id,
        environment_job_owner, environment_job_wire_kind, is_environment_job_wire_id,
        parse_environment_job_wire_id,
    },
    error::AppError,
};

/// True when a result id uses the reserved durable namespace. A malformed
/// prefixed id still routes here and is rejected, never to the legacy broker.
pub(super) fn routes_to_durable_handler(job_id: &str) -> bool {
    is_environment_job_wire_id(job_id)
}

/// Claim the next pending durable job assigned to `manager_id`, if any.
///
/// The owner-only payload is exactly what the runner-manager worker already
/// deserializes for `ensure_execution_environment`/`stop_execution_environment`
/// jobs, so no wire DTO changes are needed.
pub(super) async fn next_assigned_environment_job(
    state: &AppState,
    manager_id: i64,
) -> Result<Option<RunnerJob>, AppError> {
    let Some(row) = claim_next_assigned_manager_environment_job(&state.db, manager_id)
        .await
        .map_err(AppError::internal)?
    else {
        return Ok(None);
    };
    let kind = environment_job_wire_kind(&row).map_err(AppError::internal)?;
    Ok(Some(RunnerJob {
        job_id: encode_environment_job_wire_id(row.job_id),
        kind: kind.to_string(),
        payload: serde_json::json!({ "owner": environment_job_owner(&row) }),
    }))
}

/// Route one durable result id to the durable handler and map its typed
/// outcome to an HTTP status.
pub(super) async fn complete_environment_job_result(
    state: &AppState,
    manager_id: i64,
    result: RunnerJobResult,
) -> Result<StatusCode, AppError> {
    let Some(job_id) = parse_environment_job_wire_id(&result.job_id) else {
        return Err(AppError::bad_request(
            "invalid execution environment job id",
        ));
    };
    let outcome = complete_assigned_manager_environment_job(&state.db, manager_id, job_id, &result)
        .await
        .map_err(AppError::internal)?;
    match outcome {
        DurableJobCompletion::Completed | DurableJobCompletion::AlreadyTerminal => {
            Ok(StatusCode::NO_CONTENT)
        }
        DurableJobCompletion::NotFound => {
            Err(AppError::not_found("execution environment job not found"))
        }
        DurableJobCompletion::ForeignManager => Err(AppError::forbidden(
            "execution environment job belongs to another runner manager",
        )),
        DurableJobCompletion::Inconsistent(reason) => Err(AppError::bad_request(reason)),
        DurableJobCompletion::TerminalConflict => Err(AppError::conflict(
            "execution environment job already recorded a different outcome",
        )),
        DurableJobCompletion::NotCompletable => Err(AppError::conflict(
            "execution environment job is not claimable by this manager",
        )),
    }
}
