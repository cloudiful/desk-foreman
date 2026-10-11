//! Replacement orchestration (issue 742 P3).
//!
//! One owner-scoped transaction: a replacement is admitted only when the
//! caller's fences match the stored authorization generation, provision
//! candidate and consumed stop proof, the proof row is still a confirmed stop
//! from the original bound manager, and there is no active stop or unresolved
//! ensure blocker. The next ensure runs at the current authorization
//! generation (which must exceed the stopped candidate); the manager binding is
//! never reassigned.

use sqlx::PgPool;
use uuid::Uuid;

use crate::db::execution_environment_jobs::{ExecutionEnvironmentJob, job_token_is_valid};

use super::LifecycleError;
use super::support::{
    NewLifecycleJob, ensure_no_active_native_task, expire_overdue_jobs, find_job_by_id,
    find_job_by_token, insert_job, load_jobs, lock_environment,
};
use super::transitions::replace_update;
use super::types::{
    EnvironmentOperationDescriptor, confirmed_stop_state, environment_descriptor,
    operation_descriptor,
};

/// Admit (or idempotently replay) one replacement.
pub async fn replace_execution_environment(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    expected_generation: i64,
    expected_container_generation: i64,
    expected_stop_job_id: Uuid,
    operation_token: &str,
) -> Result<EnvironmentOperationDescriptor, LifecycleError> {
    if !job_token_is_valid(operation_token) {
        return Err(LifecycleError::InvalidToken);
    }

    let mut tx = pool.begin().await?;
    expire_overdue_jobs(&mut tx, application_id, environment_id).await?;
    let Some(environment) = lock_environment(&mut tx, application_id, environment_id).await? else {
        return Err(LifecycleError::NotFound);
    };

    // Same-token replay of an identical replacement intent returns the original
    // operation unchanged.
    if let Some(existing) =
        find_job_by_token(&mut tx, application_id, environment_id, operation_token).await?
    {
        let identical = existing.native_task_id.is_none()
            && existing.job_kind == "ensure"
            && Some(existing.assigned_manager_id) == environment.assigned_manager_id
            && Some(existing.target_generation) == environment.container_generation;
        if !identical {
            return Err(LifecycleError::TokenConflict);
        }
        return Ok(environment_descriptor(
            &environment,
            Some(&operation_descriptor(
                existing.job_id,
                &existing.job_kind,
                &existing.state,
            )),
        ));
    }

    ensure_no_active_native_task(&mut tx, application_id, environment_id).await?;
    if expected_generation != environment.generation {
        return Err(LifecycleError::StaleGeneration {
            current: environment.generation,
        });
    }
    if environment.container_generation != Some(expected_container_generation) {
        return Err(LifecycleError::StaleCandidate {
            current: environment.container_generation,
        });
    }
    let Some(proof_generation) = environment.stop_proof_generation else {
        return Err(LifecycleError::ProofMismatch);
    };
    if environment.stop_proof_job_id != Some(expected_stop_job_id) {
        return Err(LifecycleError::ProofMismatch);
    }
    if environment.generation <= proof_generation {
        // The new candidate must strictly exceed the stopped candidate.
        return Err(LifecycleError::ProofMismatch);
    }
    let Some(manager_id) = environment.assigned_manager_id else {
        return Err(LifecycleError::ProofMismatch);
    };

    // The proof row must still be a confirmed stop from the original bound
    // manager at the recorded generation; a stale, foreign or unconfirmed proof
    // is refused.
    let proof = find_job_by_id(
        &mut tx,
        application_id,
        environment_id,
        expected_stop_job_id,
    )
    .await?;
    if !proof_is_confirmed_stop(proof.as_ref(), manager_id, proof_generation) {
        return Err(LifecycleError::ProofMismatch);
    }

    // No active stop (pending/claimed) and no unresolved ensure blocker may
    // survive a replacement. A prior uncertain stop is already resolved by the
    // confirmed proof above and its history is kept.
    let jobs = load_jobs(&mut tx, application_id, environment_id).await?;
    if jobs.iter().any(|job| stop_is_active(job)) {
        return Err(LifecycleError::UnresolvedBlockers);
    }
    if jobs.iter().any(|job| ensure_is_blocker(job)) {
        return Err(LifecycleError::UnresolvedBlockers);
    }

    let new_candidate = environment.generation;
    let Some(environment) =
        replace_update(&mut tx, application_id, environment_id, new_candidate).await?
    else {
        return Err(LifecycleError::ProofMismatch);
    };

    let job = insert_job(
        &mut tx,
        NewLifecycleJob {
            token: operation_token,
            environment_id,
            application_id,
            assigned_manager_id: manager_id,
            job_kind: "ensure",
            target_generation: new_candidate,
        },
    )
    .await?;
    let Some(job) = job else {
        return Err(LifecycleError::TokenConflict);
    };

    tx.commit().await?;
    Ok(environment_descriptor(
        &environment,
        Some(&operation_descriptor(job.job_id, &job.job_kind, &job.state)),
    ))
}

pub(crate) fn proof_is_confirmed_stop(
    proof: Option<&ExecutionEnvironmentJob>,
    manager_id: i64,
    proof_generation: i64,
) -> bool {
    let Some(proof) = proof else {
        return false;
    };
    proof.job_kind == "stop"
        && proof.state == "succeeded"
        && proof.assigned_manager_id == manager_id
        && proof.target_generation == proof_generation
        && confirmed_stop_state(proof.result.as_ref()).is_some()
}

fn stop_is_active(job: &ExecutionEnvironmentJob) -> bool {
    job.native_task_id.is_none()
        && job.job_kind == "stop"
        && matches!(job.state.as_str(), "pending" | "claimed")
}

fn ensure_is_blocker(job: &ExecutionEnvironmentJob) -> bool {
    job.job_kind == "ensure" && matches!(job.state.as_str(), "pending" | "claimed" | "uncertain")
}
