//! Status reconciliation (issue 742 P3).
//!
//! One owner-scoped transaction reconciles persisted evidence for one
//! environment and returns the lifecycle descriptor. It never dispatches: the
//! row lock only covers the short reconciliation, and every fact comes from
//! durable job rows, never from wall-clock expiry or live container state. A
//! matching successful ensure marks the environment `ready` only when the
//! authorization generation equals the candidate and no stop is outstanding; a
//! matching confirmed stop records the proof, marks `replaced`, and supersedes
//! obsolete ensure records; an unconfirmed stop downgrades `stopping` to
//! `uncertain`.

use sqlx::PgPool;
use uuid::Uuid;

use crate::db::execution_environment_jobs::ExecutionEnvironmentJob;

use super::LifecycleError;
use super::support::{expire_overdue_jobs, load_jobs, lock_environment};
use super::transitions::{mark_ready, mark_replaced, mark_uncertain, supersede_ensure_jobs};
use super::types::{
    EnvironmentOperationDescriptor, LIFECYCLE_STATE_STARTING, LIFECYCLE_STATE_STOPPING,
    OperationDescriptor, confirmed_stop_state, ensure_running_result, environment_descriptor,
    operation_descriptor,
};

/// Reconcile persisted evidence and return the lifecycle descriptor.
pub async fn execution_environment_status(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
) -> Result<EnvironmentOperationDescriptor, LifecycleError> {
    let mut tx = pool.begin().await?;
    expire_overdue_jobs(&mut tx, application_id, environment_id).await?;
    let Some(mut environment) = lock_environment(&mut tx, application_id, environment_id).await?
    else {
        return Err(LifecycleError::NotFound);
    };

    let jobs = load_jobs(&mut tx, application_id, environment_id).await?;

    if let (Some(candidate), Some(manager_id)) = (
        environment.container_generation,
        environment.assigned_manager_id,
    ) {
        let confirmed_stop = jobs
            .iter()
            .filter(|job| {
                job.job_kind == "stop"
                    && job.assigned_manager_id == manager_id
                    && job.target_generation == candidate
                    && job.state == "succeeded"
                    && confirmed_stop_state(job.result.as_ref()).is_some()
            })
            .max_by_key(|job| (job.created_at, job.job_id));

        let stop_uncertain = jobs.iter().any(|job| {
            job.job_kind == "stop"
                && job.assigned_manager_id == manager_id
                && job.target_generation == candidate
                && job.state == "uncertain"
        });

        if let Some(stop) = confirmed_stop {
            if environment.state != super::types::LIFECYCLE_STATE_REPLACED
                || environment.stop_proof_job_id != Some(stop.job_id)
            {
                if let Some(updated) = mark_replaced(
                    &mut tx,
                    application_id,
                    environment_id,
                    stop.job_id,
                    candidate,
                )
                .await?
                {
                    environment = updated;
                }
            }
            // Supersede obsolete ensure records matching this proof. Repeated
            // reconciliation is a no-op once they are terminal.
            supersede_ensure_jobs(
                &mut tx,
                application_id,
                environment_id,
                manager_id,
                candidate,
            )
            .await?;
        } else if stop_uncertain && environment.state == LIFECYCLE_STATE_STOPPING {
            if let Some(updated) = mark_uncertain(&mut tx, application_id, environment_id).await? {
                environment = updated;
            }
        } else {
            let ensure_succeeded = jobs.iter().any(|job| {
                job.job_kind == "ensure"
                    && job.assigned_manager_id == manager_id
                    && job.target_generation == candidate
                    && job.state == "succeeded"
                    && ensure_running_result(job.result.as_ref())
            });
            let stop_outstanding = jobs.iter().any(|job| {
                job.job_kind == "stop"
                    && job.target_generation == candidate
                    && active_state(&job.state)
            });
            if ensure_succeeded
                && environment.generation == candidate
                && environment.state == LIFECYCLE_STATE_STARTING
                && !stop_outstanding
            {
                if let Some(updated) = mark_ready(&mut tx, application_id, environment_id).await? {
                    environment = updated;
                }
            }
        }
    }

    // Reload so the returned operation reflects any supersede just applied.
    let jobs = load_jobs(&mut tx, application_id, environment_id).await?;
    tx.commit().await?;

    let operation = newest_operation(&jobs);
    Ok(environment_descriptor(&environment, operation.as_ref()))
}

fn newest_operation(jobs: &[ExecutionEnvironmentJob]) -> Option<OperationDescriptor> {
    jobs.iter()
        .max_by_key(|job| (job.created_at, job.job_id))
        .map(|job| operation_descriptor(job.job_id, &job.job_kind, &job.state))
}

fn active_state(state: &str) -> bool {
    matches!(state, "pending" | "claimed" | "uncertain")
}
