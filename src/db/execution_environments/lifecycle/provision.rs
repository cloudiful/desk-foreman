//! First-provision orchestration (issue 742 P3).
//!
//! One owner-scoped transaction: expire overdue jobs, lock the environment
//! row, reconcile a same-token replay, refuse a duplicate candidate, bind a
//! manager on first provision only (never reassigning), record the candidate
//! generation, and enqueue one ensure intent. Async intent acknowledgment is
//! not a running confirmation.

use sqlx::PgPool;
use uuid::Uuid;

use crate::db::execution_environment_jobs::job_token_is_valid;

use super::LifecycleError;
use super::support::{
    NewLifecycleJob, ensure_no_active_native_task, expire_overdue_jobs, find_job_by_token,
    insert_job, lock_environment, select_manager,
};
use super::transitions::provision_update;
use super::types::{EnvironmentOperationDescriptor, environment_descriptor, operation_descriptor};

/// Admit one provision for `(application_id, environment_id)`.
pub async fn provision_execution_environment(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    expected_generation: i64,
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

    // A same-token replay of an identical ensure intent returns the original
    // operation unchanged; anything else under that token is a conflict.
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

    if expected_generation != environment.generation {
        return Err(LifecycleError::StaleGeneration {
            current: environment.generation,
        });
    }
    ensure_no_active_native_task(&mut tx, application_id, environment_id).await?;
    if environment.container_generation.is_some() {
        return Err(LifecycleError::AlreadyProvisioned);
    }

    // Bind a manager only on first provision; never reassign an existing
    // binding, and never fall back to another manager.
    let manager_id = match environment.assigned_manager_id {
        Some(manager_id) => manager_id,
        None => select_manager(&mut tx)
            .await?
            .ok_or(LifecycleError::ManagerUnavailable)?,
    };

    let candidate_generation = environment.generation;
    let Some(environment) = provision_update(
        &mut tx,
        application_id,
        environment_id,
        manager_id,
        candidate_generation,
    )
    .await?
    else {
        // Lost the candidate race despite the row lock; fail closed.
        return Err(LifecycleError::AlreadyProvisioned);
    };

    let job = insert_job(
        &mut tx,
        NewLifecycleJob {
            token: operation_token,
            environment_id,
            application_id,
            assigned_manager_id: manager_id,
            job_kind: "ensure",
            target_generation: candidate_generation,
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
