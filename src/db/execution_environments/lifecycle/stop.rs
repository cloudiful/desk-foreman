//! Stop orchestration (issue 742 P3).
//!
//! One owner-scoped transaction: expire overdue jobs, lock the environment row,
//! refuse a stop without a provision candidate, reconcile a same-token replay,
//! refuse a stop already in flight, and admit exactly one stop intent. The
//! first stop at the expected authorization generation increments that
//! generation once and clears any previous proof; a recovery stop after an
//! `uncertain` stop re-admits without another increment, candidate change, or
//! manager reassignment.

use sqlx::PgPool;
use uuid::Uuid;

use crate::db::execution_environment_jobs::job_token_is_valid;

use super::LifecycleError;
use super::support::{
    NewLifecycleJob, ensure_no_active_native_task, expire_overdue_jobs, find_job_by_token,
    insert_job, load_jobs, lock_environment,
};
use super::transitions::{begin_stop, resume_stop};
use super::types::{
    EnvironmentOperationDescriptor, LIFECYCLE_STATE_REPLACED, LIFECYCLE_STATE_STOPPING,
    LIFECYCLE_STATE_UNCERTAIN, environment_descriptor, operation_descriptor,
};

/// Admit (or idempotently replay) one stop for `(application_id,
/// environment_id)`. The second result is the committed candidate cutoff for a
/// new stop, and `None` for read-only replays.
pub async fn stop_execution_environment(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    expected_generation: i64,
    operation_token: &str,
) -> Result<(EnvironmentOperationDescriptor, Option<i64>), LifecycleError> {
    if !job_token_is_valid(operation_token) {
        return Err(LifecycleError::InvalidToken);
    }

    let mut tx = pool.begin().await?;
    expire_overdue_jobs(&mut tx, application_id, environment_id).await?;
    let Some(environment) = lock_environment(&mut tx, application_id, environment_id).await? else {
        return Err(LifecycleError::NotFound);
    };

    // Same-token replay of an identical stop intent never increments again.
    if let Some(existing) =
        find_job_by_token(&mut tx, application_id, environment_id, operation_token).await?
    {
        let identical = existing.native_task_id.is_none()
            && existing.job_kind == "stop"
            && Some(existing.assigned_manager_id) == environment.assigned_manager_id
            && Some(existing.target_generation) == environment.container_generation;
        if !identical {
            return Err(LifecycleError::TokenConflict);
        }
        return Ok((
            environment_descriptor(
                &environment,
                Some(&operation_descriptor(
                    existing.job_id,
                    &existing.job_kind,
                    &existing.state,
                )),
            ),
            None,
        ));
    }

    ensure_no_active_native_task(&mut tx, application_id, environment_id).await?;
    let Some(candidate) = environment.container_generation else {
        return Err(LifecycleError::NotProvisioned);
    };
    let Some(manager_id) = environment.assigned_manager_id else {
        return Err(LifecycleError::NotProvisioned);
    };
    if expected_generation != environment.generation {
        return Err(LifecycleError::StaleGeneration {
            current: environment.generation,
        });
    }
    if environment.state == LIFECYCLE_STATE_REPLACED {
        // The candidate is already durably stopped; a replacement is the next
        // valid operation, not another stop.
        return Err(LifecycleError::UnresolvedBlockers);
    }

    let jobs = load_jobs(&mut tx, application_id, environment_id).await?;
    // Only the current candidate's stops from the bound manager can describe
    // this environment's in-flight or unreconciled stop. A succeeded stop from
    // an earlier generation is historical proof and never satisfies a new stop
    // intent, so it must not short-circuit admission after a replacement.
    let latest_stop = jobs
        .iter()
        .filter(|job| {
            job.job_kind == "stop"
                && job.native_task_id.is_none()
                && job.assigned_manager_id == manager_id
                && job.target_generation == candidate
        })
        .max_by_key(|job| (job.created_at, job.job_id));
    if let Some(stop) = latest_stop {
        match stop.state.as_str() {
            "pending" | "claimed" => return Err(LifecycleError::StopInFlight),
            "succeeded" => {
                // Already confirmed and awaiting status reconciliation; do not
                // admit a second stop.
                return Ok((
                    environment_descriptor(
                        &environment,
                        Some(&operation_descriptor(
                            stop.job_id,
                            &stop.job_kind,
                            &stop.state,
                        )),
                    ),
                    None,
                ));
            }
            _ => {}
        }
    }

    let recovery = matches!(
        environment.state.as_str(),
        LIFECYCLE_STATE_STOPPING | LIFECYCLE_STATE_UNCERTAIN
    );
    let updated = if recovery {
        resume_stop(&mut tx, application_id, environment_id, expected_generation).await?
    } else {
        begin_stop(&mut tx, application_id, environment_id, expected_generation).await?
    };
    let Some(environment) = updated else {
        return Err(LifecycleError::StaleGeneration {
            current: environment.generation,
        });
    };

    let job = insert_job(
        &mut tx,
        NewLifecycleJob {
            token: operation_token,
            environment_id,
            application_id,
            assigned_manager_id: manager_id,
            job_kind: "stop",
            target_generation: candidate,
        },
    )
    .await?;
    let Some(job) = job else {
        return Err(LifecycleError::TokenConflict);
    };

    tx.commit().await?;
    Ok((
        environment_descriptor(
            &environment,
            Some(&operation_descriptor(job.job_id, &job.job_kind, &job.state)),
        ),
        Some(candidate),
    ))
}
