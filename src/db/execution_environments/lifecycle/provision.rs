//! First-provision orchestration (issue 742 P3, scoped in issue 1180 P8).
//!
//! One owner-scoped transaction: expire overdue jobs, lock the environment
//! row, reconcile a same-token replay, refuse a duplicate candidate, bind a
//! manager on first provision only (never reassigning), record the candidate
//! generation, and enqueue one ensure intent. Async intent acknowledgment is
//! not a running confirmation. A stored workspace scope additionally requires
//! authenticated authority, a fresh bound write lease, one matching live
//! manager observation and a bounded session-free observation; the unscoped
//! wrapper keeps its exact behavior.

use crate::db::task_runtime::TaskRuntime;
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::execution_environment_jobs::job_token_is_valid;

use super::LifecycleError;
use super::scope;
use super::support::{
    NewLifecycleJob, ensure_no_active_native_task, expire_overdue_jobs, find_job_by_token,
    insert_job, lock_environment, select_manager,
};
use super::transitions::provision_update;
use super::types::{
    EnvironmentOperationDescriptor, ScopedCall, environment_descriptor, operation_descriptor,
};

/// Admit one provision for `(application_id, environment_id)`.
///
/// Keeps the legacy unscoped signature. A stored workspace-scoped environment
/// is refused even on an identical replay because no authority is supplied.
pub async fn provision_execution_environment(
    pool: &PgPool,
    runtime: &TaskRuntime,
    application_id: i64,
    environment_id: Uuid,
    expected_generation: i64,
    operation_token: &str,
) -> Result<EnvironmentOperationDescriptor, LifecycleError> {
    provision_execution_environment_scoped(
        pool,
        runtime,
        application_id,
        environment_id,
        expected_generation,
        operation_token,
        None,
    )
    .await
}

/// Admit one provision with an optional authenticated workspace scope and the
/// runner seam used for the bounded pre-intent session observation.
pub async fn provision_execution_environment_scoped(
    pool: &PgPool,
    runtime: &TaskRuntime,
    application_id: i64,
    environment_id: Uuid,
    expected_generation: i64,
    operation_token: &str,
    scope: Option<ScopedCall<'_>>,
) -> Result<EnvironmentOperationDescriptor, LifecycleError> {
    if !job_token_is_valid(operation_token) {
        return Err(LifecycleError::InvalidToken);
    }

    let mut tx = pool.begin().await?;
    expire_overdue_jobs(&mut tx, runtime, application_id, environment_id).await?;
    let Some(environment) = lock_environment(&mut tx, application_id, environment_id).await? else {
        return Err(LifecycleError::NotFound);
    };

    // Resolve the stored scope and, when present, lock and validate the fresh
    // bound binding before any replay or intent decision.
    let scope_binding = scope::authorize_scope(
        &mut tx,
        application_id,
        environment_id,
        scope.as_ref().map(|scope| &scope.authority),
    )
    .await?;

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

    // Scoped admission never falls back to arbitrary manager selection: the
    // environment's own state must be absent and exactly one live matching
    // manager/root observation must authorize the intent. The unscoped path
    // keeps its existing binding-or-select behavior unchanged.
    let manager_id = match &scope_binding {
        Some(binding) => {
            if environment.state != "absent" {
                return Err(LifecycleError::InvalidState);
            }
            scope::admit_scoped_intent(
                &mut tx,
                binding,
                environment.assigned_manager_id,
                scope.as_ref().map(|scope| scope.runner),
            )
            .await?
        }
        None => match environment.assigned_manager_id {
            Some(manager_id) => manager_id,
            None => select_manager(&mut tx)
                .await?
                .ok_or(LifecycleError::ManagerUnavailable)?,
        },
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
