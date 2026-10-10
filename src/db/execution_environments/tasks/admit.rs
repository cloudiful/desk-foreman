//! Native task admission (issue 753 P1).
//!
//! One owner-scoped transaction, lock order environment then binding, both
//! held to commit. Everything that can refuse the intent is checked while
//! those locks are held, so a racing legacy write waits and then re-reads the
//! native-active state instead of trusting the snapshot it started from.

use sqlx::PgPool;
use uuid::Uuid;

use super::queries::{
    NewNativeTask, find_active_task_for_binding, find_task, find_task_by_token, insert_task,
    lock_binding, lock_environment, set_environment_active_task,
};
use super::types::{NativeTask, NativeTaskError, NativeTaskRequest, TaskBinding, TaskEnvironment};
use super::{internal, resolve_task_manager, task_token_is_valid};
use crate::db::execution_environments::lifecycle::{
    expire_overdue_jobs, find_job_by_id, load_jobs, proof_is_confirmed_stop,
};

/// Whether the environment is in a shape that admits a task intent.
///
/// Exactly two shapes qualify:
///
/// - no candidate generation at all, so nothing is running for this
///   generation;
/// - a candidate generation released by its consumed stop proof, which must
///   still be a confirmed stop from the environment's own immutable manager at
///   that exact candidate generation.
///
/// A `ready` or `running` candidate is never admitted: its live container has
/// mounts this phase cannot reason about, so it is refused rather than silently
/// adopted. The confirmed-stop rule is the same one replacement applies.
async fn admit_shape_is_eligible(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    application_id: i64,
    environment_id: Uuid,
    environment: &TaskEnvironment,
) -> Result<bool, NativeTaskError> {
    let Some(container_generation) = environment.container_generation else {
        return Ok(true);
    };
    if environment.state != "replaced" {
        return Ok(false);
    }
    let (Some(stop_proof_generation), Some(stop_proof_job_id)) = (
        environment.stop_proof_generation,
        environment.stop_proof_job_id,
    ) else {
        return Ok(false);
    };
    // The proof must be for the candidate the environment actually carries.
    if stop_proof_generation != container_generation {
        return Ok(false);
    }
    // The task will bind to this manager, so the proof must be the one that
    // manager produced; an environment with a candidate but no manager is
    // inconsistent and is refused rather than guessed at.
    let Some(manager_id) = environment.assigned_manager_id else {
        return Ok(false);
    };
    let proof = find_job_by_id(tx, application_id, environment_id, stop_proof_job_id)
        .await
        .map_err(internal)?;
    Ok(proof_is_confirmed_stop(
        proof.as_ref(),
        manager_id,
        stop_proof_generation,
    ))
}

pub(super) async fn admit(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    request: NativeTaskRequest,
) -> Result<NativeTask, NativeTaskError> {
    if !task_token_is_valid(&request.operation_token) {
        return Err(NativeTaskError::InvalidToken);
    }

    let mut tx = pool.begin().await.map_err(internal)?;
    // Overdue lifecycle rows fail closed to `uncertain` before any decision, so
    // an expired ensure can never read as "no blockers".
    expire_overdue_jobs(&mut tx, application_id, environment_id)
        .await
        .map_err(internal)?;

    let environment = lock_environment(&mut tx, application_id, environment_id)
        .await
        .map_err(internal)?
        .ok_or(NativeTaskError::NotFound)?;

    // Same-token replay of an identical intent returns the original task; a
    // different intent under the same token is a conflict.
    if let Some(existing) = find_task_by_token(&mut tx, application_id, &request.operation_token)
        .await
        .map_err(internal)?
    {
        let identical = existing.task_id == request.task_id
            && existing.environment_id == environment_id
            && existing.workspace_binding_id == request.workspace_binding_id
            && existing.target_generation == request.expected_generation;
        if !identical {
            return Err(NativeTaskError::TokenConflict);
        }
        tx.commit().await.map_err(internal)?;
        return Ok(existing);
    }

    if request.expected_generation != environment.generation {
        return Err(NativeTaskError::StaleGeneration);
    }
    if request.subject_external_user_id != environment.external_user_id {
        // Authorization is the authenticated application's delegation; the
        // subject must match the environment's own subject. The fixed resource
        // marker is never compared against it.
        return Err(NativeTaskError::SubjectMismatch);
    }
    if environment.active_native_task_id.is_some() {
        return Err(NativeTaskError::TaskAlreadyActive);
    }
    if !admit_shape_is_eligible(&mut tx, application_id, environment_id, &environment).await? {
        return Err(NativeTaskError::EnvironmentNotIdle);
    }

    let binding = lock_binding(&mut tx, request.workspace_binding_id)
        .await
        .map_err(internal)?
        .ok_or(NativeTaskError::NotFound)?;
    check_binding(&binding, application_id, &request)?;
    // A shared workspace carries at most one active task, across environments.
    // The binding row is locked here, so this read is fresh: a task admitted
    // while we waited is visible and refused as a conflict rather than
    // colliding on the partial unique index at insert time.
    if find_active_task_for_binding(&mut tx, request.workspace_binding_id)
        .await
        .map_err(internal)?
        .is_some()
    {
        return Err(NativeTaskError::TaskAlreadyActive);
    }

    // No unresolved ensure/stop operation may survive an admission.
    let jobs = load_jobs(&mut tx, application_id, environment_id)
        .await
        .map_err(internal)?;
    if jobs
        .iter()
        .any(|job| matches!(job.state.as_str(), "pending" | "claimed" | "uncertain"))
    {
        return Err(NativeTaskError::UnresolvedBlockers);
    }

    let manager_id = resolve_task_manager(&mut tx, environment.assigned_manager_id).await?;
    let Some(manager_id) = manager_id else {
        return Err(NativeTaskError::NoManagerAvailable);
    };

    let admitted = insert_task(
        &mut tx,
        NewNativeTask {
            task_id: request.task_id,
            operation_token: &request.operation_token,
            environment_id,
            application_id,
            workspace_binding_id: request.workspace_binding_id,
            subject_external_user_id: &request.subject_external_user_id,
            target_generation: environment.generation,
            assigned_manager_id: manager_id,
        },
    )
    .await
    .map_err(internal)?;

    // A conflict on either uniqueness constraint returns no row; resolve it
    // inside this transaction before touching the environment anchor. The
    // statement waited for any concurrent conflicting writer to finish, so the
    // winner is committed and visible here.
    let Some(admitted) = admitted else {
        // The resolver reports the winner or a typed refusal; the caller owns
        // the commit either way.
        let resolved =
            resolve_insert_conflict(&mut tx, application_id, environment_id, &request).await?;
        tx.commit().await.map_err(internal)?;
        return Ok(resolved);
    };

    // The task row and the environment's active anchor commit together: a
    // task that exists without its anchor would let a second task in.
    let anchored =
        set_environment_active_task(&mut tx, application_id, environment_id, request.task_id)
            .await
            .map_err(internal)?;
    if !anchored {
        return Err(NativeTaskError::TaskAlreadyActive);
    }

    tx.commit().await.map_err(internal)?;
    Ok(admitted)
}

/// Resolve an insert that conflicted on a uniqueness constraint.
///
/// The two keys are resolved in order of strength, and every branch stays
/// application-scoped so a foreign token or task id is indistinguishable from
/// an absent one:
///
/// - an existing row under *our* operation token is an exact intent replay when
///   it matches, and a `TokenConflict` when it does not;
/// - otherwise a visible row under the same `task_id` means the slot is taken;
/// - otherwise some other tenant holds a key we cannot see, and the caller gets
///   a generic conflict that confirms nothing.
async fn resolve_insert_conflict(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    application_id: i64,
    environment_id: Uuid,
    request: &NativeTaskRequest,
) -> Result<NativeTask, NativeTaskError> {
    if let Some(existing) = find_task_by_token(tx, application_id, &request.operation_token)
        .await
        .map_err(internal)?
    {
        let identical = existing.task_id == request.task_id
            && existing.environment_id == environment_id
            && existing.workspace_binding_id == request.workspace_binding_id
            && existing.target_generation == request.expected_generation;
        if identical {
            return Ok(existing);
        }
        return Err(NativeTaskError::TokenConflict);
    }

    if find_task(tx, application_id, environment_id, request.task_id)
        .await
        .map_err(internal)?
        .is_some()
    {
        return Err(NativeTaskError::TaskAlreadyActive);
    }

    // Neither key is visible to this application: a foreign tenant owns one of
    // them. Refuse generically rather than report what we cannot see.
    Err(NativeTaskError::TokenConflict)
}

/// Apply every binding eligibility rule while the binding row is locked.
fn check_binding(
    binding: &TaskBinding,
    application_id: i64,
    request: &NativeTaskRequest,
) -> Result<(), NativeTaskError> {
    if binding.application_id != application_id {
        return Err(NativeTaskError::NotFound);
    }
    if !binding.is_active_code_project() {
        return Err(NativeTaskError::BindingIneligible);
    }
    if binding.lifecycle_blocks_admission() {
        return Err(NativeTaskError::BindingLifecycle);
    }
    if binding.has_legacy_write_owner() {
        // Any owner blocks, expired or not: an expiry is a cleanup timer, not
        // evidence that no dispatched process is still writing.
        return Err(NativeTaskError::LegacyOwnerPresent);
    }
    if binding.workspace_binding_id != request.workspace_binding_id {
        return Err(NativeTaskError::NotFound);
    }
    Ok(())
}
