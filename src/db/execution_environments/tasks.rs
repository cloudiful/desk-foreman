//! Durable native task ownership and original-manager execution control (issue 764 P1).
//!
//! An authenticated application admits one task per environment and authorized
//! workspace binding, prepares its sealed seed, schedules a fenced start and a
//! task-aware stop, and releases only against the task's own positive
//! environment and preparation-quiescence proof. Manager-side native
//! start/stop execution and default service routing remain outside this phase.

mod admit;
mod cancel;
pub(crate) mod native_execution;
pub(crate) mod prepare;
mod queries;
mod release;
pub mod seeds;
mod start;
mod stop;
mod types;

use sqlx::PgPool;
use uuid::Uuid;

pub use native_execution::{
    NativeTaskOperationDescriptor, TaskExecutionError, TaskExecutionRequest,
};
pub use prepare::{
    PreparationDescriptor, PrepareError, PrepareRequest, find_native_task_preparation,
    prepare_native_task,
};
pub use types::{
    NATIVE_TASK_ACTIVE_STATES, NATIVE_TASK_STATES, NativeTask, NativeTaskError, NativeTaskRequest,
    TaskBinding, TaskEnvironment, TaskStopProof, task_subject_is_valid, task_token_is_valid,
};

pub use seeds::preparation_seed_for_manager;

use crate::db::execution_environments::lifecycle::select_manager;

/// Admit (or idempotently replay) one durable task intent.
///
/// Lock order is environment then binding, and both are held until the intent
/// commits: a racing legacy write waits on the binding row and then re-reads
/// the native-active state, so it can never step through a stale snapshot.
pub async fn admit_native_task(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    request: NativeTaskRequest,
) -> Result<NativeTask, NativeTaskError> {
    admit::admit(pool, application_id, environment_id, request).await
}

/// Read one owned task intent.
pub async fn find_native_task(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
) -> Result<NativeTask, NativeTaskError> {
    let mut tx = pool.begin().await.map_err(internal)?;
    let task = queries::find_task(&mut tx, application_id, environment_id, task_id)
        .await
        .map_err(internal)?;
    tx.commit().await.map_err(internal)?;
    task.ok_or(NativeTaskError::NotFound)
}

/// Cancel one task intent at the caller's current generation fence.
///
/// A task that provably never started is voided and frees its environment and
/// binding. A task with any candidate or dispatched job reference is marked
/// `uncertain` instead: it may have a writer, so it keeps blocking until an
/// operator resolves it.
pub async fn cancel_native_task(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
    expected_generation: i64,
) -> Result<NativeTask, NativeTaskError> {
    cancel::cancel(
        pool,
        application_id,
        environment_id,
        task_id,
        expected_generation,
    )
    .await
}

/// Release an executed task against its own confirmed stop proof.
///
/// Every fence is checked in one transaction: the current generation, the
/// task's container generation, the environment's active-task anchor, and a
/// typed stop proof that is this task's, from this task's original manager, at
/// this task's target generation, recording positive quiescence. Anything
/// else leaves the task untouched.
pub async fn release_native_task(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
    expected_generation: i64,
    expected_container_generation: i64,
    expected_stop_job_id: Uuid,
) -> Result<NativeTask, NativeTaskError> {
    release::release(
        pool,
        application_id,
        environment_id,
        task_id,
        expected_generation,
        expected_container_generation,
        expected_stop_job_id,
    )
    .await
}

/// Schedule or replay the original-manager start for a prepared native task.
pub async fn start_native_task(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
    request: TaskExecutionRequest,
) -> Result<NativeTaskOperationDescriptor, TaskExecutionError> {
    start::start(pool, application_id, environment_id, task_id, request).await
}

/// Schedule or replay the original-manager stop for a native task.
pub async fn stop_native_task(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
    request: TaskExecutionRequest,
) -> Result<NativeTaskOperationDescriptor, TaskExecutionError> {
    stop::stop(pool, application_id, environment_id, task_id, request).await
}

/// Wrap a query failure as an internal error without leaking its text.
///
/// The failure is logged with its detail and reported to the caller as a bare
/// `Internal`, so a database message can never reach a response body.
pub(super) fn internal(error: impl std::fmt::Display) -> NativeTaskError {
    tracing::error!(error = %error, "native task persistence failed");
    NativeTaskError::Internal
}

/// Resolve the manager a fresh task binds to: the environment's immutable
/// binding if it has one, otherwise the internal selection used by first
/// provision. A task never re-binds to a replacement manager later.
pub(super) async fn resolve_task_manager(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    assigned_manager_id: Option<i64>,
) -> Result<Option<i64>, NativeTaskError> {
    match assigned_manager_id {
        Some(manager_id) => Ok(Some(manager_id)),
        None => select_manager(tx).await.map_err(internal),
    }
}
