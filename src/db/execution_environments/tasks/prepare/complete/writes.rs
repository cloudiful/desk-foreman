//! Static-query writes used by atomic preparation completion.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::db::execution_environment_jobs::ExecutionEnvironmentJob;
use crate::db::execution_environments::NativeTask;

pub(super) async fn complete_job(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
    manager_id: i64,
    state: &str,
    result: serde_json::Value,
    error_summary: Option<&str>,
) -> anyhow::Result<Option<ExecutionEnvironmentJob>> {
    Ok(sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environments/tasks/prepare/complete_preparation_job.sql",
        job_id,
        application_id,
        environment_id,
        state,
        result,
        error_summary,
        manager_id,
        task_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

pub(super) async fn mark_task_prepared(
    tx: &mut Transaction<'_, Postgres>,
    task_id: Uuid,
    application_id: i64,
    environment_id: Uuid,
    job_id: Uuid,
) -> anyhow::Result<Option<NativeTask>> {
    Ok(sqlx::query_file_as!(
        NativeTask,
        "src/sql/execution_environments/tasks/prepare/mark_task_prepared.sql",
        task_id,
        application_id,
        environment_id,
        job_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}
