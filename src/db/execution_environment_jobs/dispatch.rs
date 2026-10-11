//! Assigned-manager dispatch for durable environment jobs (issue 742 P2).
//!
//! Two directions share one reserved wire namespace, `envjob:<UUID>`, which
//! keeps durable jobs disjoint from the legacy in-memory broker's plain UUID
//! ids. Pull claims the oldest pending row assigned to the authenticated
//! manager with one atomic compare-and-set; result handling looks the row up
//! by its UUID, re-checks the assigned manager, validates the reported ack
//! against the row's exact owner through [`super::resolution`], and only then
//! records a terminal outcome through the existing completion compare-and-set.
//!
//! Nothing here trusts a client-supplied owner, application, or token: the
//! row is the authority, and the worker manager identity is checked before any
//! evaluation. A malformed or inconsistent ack mutates no row.

use runner_protocol::{NativeTaskPreparedResult, RunnerJobResult};
use sqlx::PgPool;
use uuid::Uuid;

use super::lifecycle::{CompleteOutcome, complete_execution_environment_job};
use super::resolution::{EnvironmentJobResolution, resolve_environment_job_result};
use super::types::ExecutionEnvironmentJob;

/// Reserved opaque wire-id prefix for durable environment jobs. Legacy broker
/// ids are plain UUIDs, so the namespace is unambiguous.
pub(crate) const ENV_JOB_ID_PREFIX: &str = "envjob:";

/// Encode a durable job UUID into the reserved opaque wire id.
pub(crate) fn encode_environment_job_wire_id(job_id: Uuid) -> String {
    format!("{ENV_JOB_ID_PREFIX}{job_id}")
}

/// True when a wire result id uses the reserved durable namespace. A malformed
/// prefixed id still routes here (and is rejected), never to the legacy broker.
pub(crate) fn is_environment_job_wire_id(job_id: &str) -> bool {
    job_id.starts_with(ENV_JOB_ID_PREFIX)
}

/// Parse a reserved durable wire id into its row UUID, or `None` when it is
/// malformed.
pub(crate) fn parse_environment_job_wire_id(job_id: &str) -> Option<Uuid> {
    job_id
        .strip_prefix(ENV_JOB_ID_PREFIX)
        .and_then(|raw| Uuid::parse_str(raw).ok())
}

/// Terminal outcome of completing one durable job through the store CAS.
#[derive(Debug)]
pub(crate) enum DurableJobCompletion {
    /// The claimant recorded a terminal result.
    Completed,
    /// An identical already-terminal replay.
    AlreadyTerminal,
    /// A replay that contradicts the recorded terminal outcome.
    TerminalConflict,
    /// No current claim matched; nothing was mutated.
    NotCompletable,
    /// No such durable job.
    NotFound,
    /// The authenticated manager is not the assigned manager.
    ForeignManager,
    /// The reported ack was malformed or inconsistent; nothing was mutated.
    Inconsistent(&'static str),
}

/// Claim the oldest pending durable job assigned to `manager_id`, atomically
/// marking it claimed. `None` means no claimable job; claimed, uncertain, and
/// expired rows are never returned.
pub(crate) async fn claim_next_assigned_manager_environment_job(
    pool: &PgPool,
    manager_id: i64,
) -> anyhow::Result<Option<ExecutionEnvironmentJob>> {
    sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environment_jobs/claim_next_for_assigned_manager.sql",
        manager_id,
    )
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

/// Mark the linked task uncertain and the job uncertain after a failed or
/// unconfirmed preparation. The task keeps its exclusive anchors and is never
/// re-dispatched.
async fn complete_preparation_failure(
    pool: &PgPool,
    row: &ExecutionEnvironmentJob,
) -> anyhow::Result<DurableJobCompletion> {
    let outcome =
        crate::db::execution_environments::tasks::prepare::complete_native_task_preparation(
            pool,
            crate::db::execution_environments::tasks::prepare::PreparationCompletion {
                manager_id: row.assigned_manager_id,
                job_id: row.job_id,
                result: None,
                failed: true,
            },
        )
        .await?;
    Ok(match outcome {
        crate::db::execution_environments::tasks::prepare::PreparationCompletionOutcome::Uncertain
        | crate::db::execution_environments::tasks::prepare::PreparationCompletionOutcome::Prepared => {
            DurableJobCompletion::Completed
        }
        crate::db::execution_environments::tasks::prepare::PreparationCompletionOutcome::Inconsistent(
            reason,
        ) => DurableJobCompletion::Inconsistent(reason),
        crate::db::execution_environments::tasks::prepare::PreparationCompletionOutcome::NotFound => {
            DurableJobCompletion::NotFound
        }
        crate::db::execution_environments::tasks::prepare::PreparationCompletionOutcome::ForeignManager => {
            DurableJobCompletion::ForeignManager
        }
        crate::db::execution_environments::tasks::prepare::PreparationCompletionOutcome::NotCompletable => {
            DurableJobCompletion::NotCompletable
        }
    })
}

/// Trusted manager-only lookup of one durable job by its stable UUID.
pub(crate) async fn find_execution_environment_job_by_id(
    pool: &PgPool,
    job_id: Uuid,
) -> anyhow::Result<Option<ExecutionEnvironmentJob>> {
    sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environment_jobs/find_execution_environment_job_by_id.sql",
        job_id,
    )
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

/// Complete one durable job reported by the authenticated manager. The row is
/// the authority for owner pair, token, and manager; the reported ack must
/// match its exact owner before a terminal outcome is recorded.
pub(crate) async fn complete_assigned_manager_environment_job(
    pool: &PgPool,
    manager_id: i64,
    job_id: Uuid,
    result: &RunnerJobResult,
) -> anyhow::Result<DurableJobCompletion> {
    let Some(row) = find_execution_environment_job_by_id(pool, job_id).await? else {
        return Ok(DurableJobCompletion::NotFound);
    };
    if row.assigned_manager_id != manager_id {
        return Ok(DurableJobCompletion::ForeignManager);
    }

    if row.job_kind == "start_native_task"
        || (row.job_kind == "stop" && row.native_task_id.is_some())
    {
        use crate::db::execution_environments::tasks::native_execution::NativeJobCompletion;
        let outcome =
            crate::db::execution_environments::tasks::native_execution::complete_native_task_job(
                pool, manager_id, job_id, result,
            )
            .await?;
        return Ok(match outcome {
            NativeJobCompletion::Completed => DurableJobCompletion::Completed,
            NativeJobCompletion::AlreadyTerminal => DurableJobCompletion::AlreadyTerminal,
            NativeJobCompletion::NotCompletable => DurableJobCompletion::NotCompletable,
            NativeJobCompletion::NotFound => DurableJobCompletion::NotFound,
            NativeJobCompletion::ForeignManager => DurableJobCompletion::ForeignManager,
            NativeJobCompletion::Inconsistent(reason) => DurableJobCompletion::Inconsistent(reason),
        });
    }

    // A preparation is not an environment operation: its ack is a typed proof
    // about the task's sealed seed, so it takes a DB-aware path rather than the
    // pure owner/container evaluation below.
    if row.job_kind == "prepare_native_task" {
        if !result.ok {
            return complete_preparation_failure(pool, &row).await;
        }
        let Some(value) = result.result.clone() else {
            return Ok(DurableJobCompletion::Inconsistent(
                "prepared result carried no payload",
            ));
        };
        let Ok(ack) = serde_json::from_value::<NativeTaskPreparedResult>(value) else {
            return Ok(DurableJobCompletion::Inconsistent(
                "prepared ack could not be decoded",
            ));
        };
        let outcome =
            crate::db::execution_environments::tasks::prepare::complete_native_task_preparation(
                pool,
                crate::db::execution_environments::tasks::prepare::PreparationCompletion {
                    manager_id,
                    job_id: row.job_id,
                    result: Some(ack),
                    failed: false,
                },
            )
            .await?;
        return Ok(match outcome {
            crate::db::execution_environments::tasks::prepare::PreparationCompletionOutcome::Prepared
            | crate::db::execution_environments::tasks::prepare::PreparationCompletionOutcome::Uncertain => {
                DurableJobCompletion::Completed
            }
            crate::db::execution_environments::tasks::prepare::PreparationCompletionOutcome::Inconsistent(
                reason,
            ) => DurableJobCompletion::Inconsistent(reason),
            crate::db::execution_environments::tasks::prepare::PreparationCompletionOutcome::NotFound => {
                DurableJobCompletion::NotFound
            }
            crate::db::execution_environments::tasks::prepare::PreparationCompletionOutcome::ForeignManager => {
                DurableJobCompletion::ForeignManager
            }
            crate::db::execution_environments::tasks::prepare::PreparationCompletionOutcome::NotCompletable => {
                DurableJobCompletion::NotCompletable
            }
        });
    }

    let (state, job_result, error_summary) = match resolve_environment_job_result(&row, result) {
        EnvironmentJobResolution::Inconsistent(reason) => {
            return Ok(DurableJobCompletion::Inconsistent(reason));
        }
        EnvironmentJobResolution::Succeeded(job_result) => ("succeeded", job_result, None),
        EnvironmentJobResolution::Uncertain(job_result, diagnostic) => {
            ("uncertain", job_result, Some(diagnostic))
        }
    };

    let outcome = complete_execution_environment_job(
        pool,
        row.application_id,
        row.environment_id,
        &row.token,
        row.job_id,
        manager_id,
        state,
        Some(job_result),
        error_summary,
    )
    .await?;
    Ok(match outcome {
        CompleteOutcome::Completed(_) => DurableJobCompletion::Completed,
        CompleteOutcome::AlreadyTerminal(_) => DurableJobCompletion::AlreadyTerminal,
        CompleteOutcome::TerminalConflict { .. } => DurableJobCompletion::TerminalConflict,
        CompleteOutcome::NotCompletable { .. } => DurableJobCompletion::NotCompletable,
    })
}
