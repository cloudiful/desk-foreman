//! Native task cancellation (issue 753 P1).
//!
//! Cancel is a decision about proof, not about intent. A task with no
//! candidate generation and no dispatched job reference provably never
//! started, so it is voided and its environment and binding are freed. Any
//! other task may already have a writer, so it is marked `uncertain`: both
//! active indexes keep counting it, and it keeps blocking until an operator
//! resolves it. No stop is inferred or dispatched here.

use sqlx::PgPool;
use uuid::Uuid;

use super::internal;
use super::queries::{
    clear_environment_active_task, count_job_references, lock_environment, lock_task,
    mark_uncertain, mark_voided,
};
use super::types::{NativeTask, NativeTaskError};

pub(super) async fn cancel(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
    expected_generation: i64,
) -> Result<NativeTask, NativeTaskError> {
    let mut tx = pool.begin().await.map_err(internal)?;

    let environment = lock_environment(&mut tx, application_id, environment_id)
        .await
        .map_err(internal)?
        .ok_or(NativeTaskError::NotFound)?;
    if expected_generation != environment.generation {
        return Err(NativeTaskError::StaleGeneration);
    }

    let task = lock_task(&mut tx, application_id, environment_id, task_id)
        .await
        .map_err(internal)?
        .ok_or(NativeTaskError::NotFound)?;

    // Cancelling someone else's task is a not-found, never a conflict that
    // would confirm the task exists.
    if task.is_released() {
        // Already terminal: idempotent, and no other owner's state changes.
        tx.commit().await.map_err(internal)?;
        return Ok(task);
    }
    if environment.active_native_task_id != Some(task.task_id) {
        return Err(NativeTaskError::NotFound);
    }

    // A candidate generation or any dispatched job means a writer may exist.
    let dispatched = count_job_references(&mut tx, task_id)
        .await
        .map_err(internal)?;
    let may_have_started = task.container_generation.is_some() || dispatched > 0;

    let outcome = if may_have_started {
        mark_uncertain(&mut tx, application_id, environment_id, task_id)
            .await
            .map_err(internal)?
            .ok_or(NativeTaskError::NotFound)?
    } else {
        let voided = mark_voided(&mut tx, application_id, environment_id, task_id)
            .await
            .map_err(internal)?
            .ok_or(NativeTaskError::NotFound)?;
        // Only a provably-never-started task frees the environment anchor;
        // an uncertain task deliberately keeps it.
        clear_environment_active_task(&mut tx, application_id, environment_id, task_id)
            .await
            .map_err(internal)?;
        voided
    };

    tx.commit().await.map_err(internal)?;
    Ok(outcome)
}
