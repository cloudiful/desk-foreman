use super::completion_resolution::{START_FAILURE, STOP_FAILURE};
use crate::db::execution_environment_jobs::{
    ExecutionEnvironmentJob, ExecutionEnvironmentJobResult,
};

pub(super) async fn mark_uncertain(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    environment: &crate::db::execution_environments::tasks::TaskEnvironment,
    task: &crate::db::execution_environments::NativeTask,
    job: &ExecutionEnvironmentJob,
) -> anyhow::Result<()> {
    let task_id = task.task_id;
    let task_result = if job.job_kind == "start_native_task" {
        sqlx::query_file_scalar!(
            "src/sql/execution_environments/tasks/start/mark_native_task_uncertain.sql",
            task_id,
            task.application_id,
            task.environment_id,
            job.job_id,
        )
        .fetch_optional(&mut **tx)
        .await?
    } else {
        sqlx::query_file_scalar!(
            "src/sql/execution_environments/tasks/stop/mark_native_task_uncertain.sql",
            task_id,
            task.application_id,
            task.environment_id,
            job.job_id,
        )
        .fetch_optional(&mut **tx)
        .await?
    };
    let environment_result = if job.job_kind == "start_native_task" {
        sqlx::query_file_scalar!(
            "src/sql/execution_environments/tasks/start/mark_environment_uncertain_for_task.sql",
            environment.environment_id,
            environment.application_id,
            task_id,
            job.target_generation,
        )
        .fetch_optional(&mut **tx)
        .await?
    } else {
        sqlx::query_file_scalar!(
            "src/sql/execution_environments/tasks/stop/mark_environment_uncertain_for_task.sql",
            environment.environment_id,
            environment.application_id,
            task_id,
            job.target_generation,
        )
        .fetch_optional(&mut **tx)
        .await?
    };
    if task_result.is_none() || environment_result.is_none() {
        anyhow::bail!("native task uncertainty transition lost its fence");
    }
    Ok(())
}

pub(super) async fn expire_locked_job(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    environment: &crate::db::execution_environments::tasks::TaskEnvironment,
    task: &crate::db::execution_environments::NativeTask,
    job: &ExecutionEnvironmentJob,
) -> anyhow::Result<()> {
    let result = serde_json::to_value(ExecutionEnvironmentJobResult {
        outcome: if job.job_kind == "start_native_task" {
            "native_task_start_uncertain".to_string()
        } else {
            "stop_unconfirmed".to_string()
        },
        observed_state: None,
        confirmed: Some(false),
        preparation_quiescent: if job.job_kind == "stop" {
            Some(false)
        } else {
            None
        },
    })?;
    mark_uncertain(tx, environment, task, job).await?;
    let updated = sqlx::query_file_scalar!(
        "src/sql/execution_environment_jobs/native_execution/expire_native_execution_job.sql",
        job.job_id,
        job.application_id,
        job.environment_id,
        task.task_id,
        result,
        if job.job_kind == "start_native_task" {
            START_FAILURE
        } else {
            STOP_FAILURE
        },
    )
    .fetch_optional(&mut **tx)
    .await?;
    if updated.is_none() {
        anyhow::bail!("native execution deadline changed while locked");
    }
    Ok(())
}
