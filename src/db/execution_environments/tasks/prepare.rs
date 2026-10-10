//! Durable native-task preparation (issue 753 P2b).
//!
//! Scheduling records a durable intent for the task's original manager to
//! materialize the task's sealed seed into a private checkout. This module is
//! control-plane only: it enqueues and reads the durable job, moves the task to
//! `preparing`, and never dispatches, mounts or starts anything.

mod complete;
mod schedule;
mod types;

use sqlx::PgPool;
use uuid::Uuid;

pub use types::{PreparationDescriptor, PrepareError, PrepareRequest};

pub(crate) use complete::{
    PreparationCompletion, PreparationCompletionOutcome, complete_native_task_preparation,
    mark_preparation_uncertain,
};

use crate::db::execution_environments::tasks::queries::{lock_environment, lock_task};

/// Schedule (or idempotently replay) one durable preparation.
pub async fn prepare_native_task(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
    request: PrepareRequest,
) -> Result<PreparationDescriptor, PrepareError> {
    schedule::schedule(pool, application_id, environment_id, task_id, &request).await
}

/// Read the durable preparation of one owned task.
pub async fn find_native_task_preparation(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
) -> Result<PreparationDescriptor, PrepareError> {
    let mut tx = pool.begin().await.map_err(types::internal)?;
    lock_environment(&mut tx, application_id, environment_id)
        .await
        .map_err(types::internal)?
        .ok_or(PrepareError::NotFound)?;
    let task = lock_task(&mut tx, application_id, environment_id, task_id)
        .await
        .map_err(types::internal)?
        .ok_or(PrepareError::NotFound)?;
    let job = sqlx::query_file_as!(
        crate::db::execution_environment_jobs::ExecutionEnvironmentJob,
        "src/sql/execution_environments/tasks/prepare/find_preparation_by_task.sql",
        task_id,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(types::internal)?
    .ok_or(PrepareError::NotFound)?;
    let descriptor = PreparationDescriptor {
        task_id,
        task_state: task.state.clone(),
        job_id: job.job_id,
        job_state: job.state.clone(),
        target_generation: job.target_generation,
        active: crate::db::execution_environments::NATIVE_TASK_ACTIVE_STATES
            .contains(&task.state.as_str()),
        released: task.state == "released",
    };
    tx.commit().await.map_err(types::internal)?;
    Ok(descriptor)
}
