use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::types::TaskExecutionError;
use crate::db::{
    execution_environment_jobs::ExecutionEnvironmentJob,
    execution_environments::tasks::{NativeTask, TaskBinding, TaskEnvironment},
};

pub(crate) async fn lock_context(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
) -> Result<(TaskEnvironment, NativeTask, TaskBinding), TaskExecutionError> {
    let environment = crate::db::execution_environments::tasks::queries::lock_environment(
        tx,
        application_id,
        environment_id,
    )
    .await
    .map_err(super::types::internal)?
    .ok_or(TaskExecutionError::NotFound)?;
    let task = crate::db::execution_environments::tasks::queries::lock_task(
        tx,
        application_id,
        environment_id,
        task_id,
    )
    .await
    .map_err(super::types::internal)?
    .ok_or(TaskExecutionError::NotFound)?;
    let binding = crate::db::execution_environments::tasks::queries::lock_binding(
        tx,
        task.workspace_binding_id,
    )
    .await
    .map_err(super::types::internal)?
    .ok_or(TaskExecutionError::NotFound)?;
    if binding.application_id != application_id {
        return Err(TaskExecutionError::NotFound);
    }
    Ok((environment, task, binding))
}

pub(crate) async fn lock_job(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    job_id: Uuid,
) -> Result<Option<ExecutionEnvironmentJob>, TaskExecutionError> {
    sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environment_jobs/native_execution/lock_native_execution_job.sql",
        job_id,
        application_id,
        environment_id,
    )
    .fetch_optional(&mut **tx)
    .await
    .map_err(super::types::internal)
}

pub(crate) async fn find_job(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    job_id: Uuid,
) -> Result<Option<ExecutionEnvironmentJob>, TaskExecutionError> {
    crate::db::execution_environments::lifecycle::find_job_by_id(
        tx,
        application_id,
        environment_id,
        job_id,
    )
    .await
    .map_err(super::types::internal)
}

pub(crate) async fn find_job_by_token(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    token: &str,
) -> Result<Option<ExecutionEnvironmentJob>, TaskExecutionError> {
    crate::db::execution_environments::lifecycle::find_job_by_token(
        tx,
        application_id,
        environment_id,
        token,
    )
    .await
    .map_err(super::types::internal)
}

pub(crate) async fn load_jobs(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
) -> Result<Vec<ExecutionEnvironmentJob>, TaskExecutionError> {
    crate::db::execution_environments::lifecycle::load_jobs(tx, application_id, environment_id)
        .await
        .map_err(super::types::internal)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_job(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    token: &str,
    environment_id: Uuid,
    application_id: i64,
    manager_id: i64,
    job_kind: &str,
    target_generation: i64,
    task_id: Uuid,
    admission_generation: Option<i64>,
    deadline_at: chrono::DateTime<chrono::Utc>,
) -> Result<Option<ExecutionEnvironmentJob>, TaskExecutionError> {
    sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environment_jobs/native_execution/insert_native_execution_job.sql",
        job_id,
        token,
        environment_id,
        application_id,
        manager_id,
        job_kind,
        target_generation,
        task_id,
        deadline_at,
        runner_protocol::RUNNER_MANAGER_HEARTBEAT_TTL_SECS as f64,
        admission_generation,
    )
    .fetch_optional(&mut **tx)
    .await
    .map_err(super::types::internal)
}

pub(crate) async fn manager_is_usable(
    tx: &mut Transaction<'_, Postgres>,
    manager_id: i64,
) -> Result<bool, TaskExecutionError> {
    let row: Option<i64> = sqlx::query_file_scalar!(
        "src/sql/execution_environments/tasks/prepare/is_manager_usable.sql",
        manager_id,
        runner_protocol::RUNNER_MANAGER_HEARTBEAT_TTL_SECS as f64,
    )
    .fetch_optional(&mut **tx)
    .await
    .map_err(super::types::internal)?;
    Ok(row.is_some())
}

pub(crate) async fn seed_identity(
    tx: &mut Transaction<'_, Postgres>,
    task_id: Uuid,
) -> Result<Option<(String, u32, u32)>, TaskExecutionError> {
    crate::db::execution_environments::tasks::seeds::find_native_task_seed_identity(tx, task_id)
        .await
        .map_err(super::types::internal)
}
