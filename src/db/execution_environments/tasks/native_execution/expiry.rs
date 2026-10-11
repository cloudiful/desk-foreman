use sqlx::{Postgres, Transaction};

use super::queries::{lock_context, lock_job};
use crate::db::execution_environment_jobs::{
    ExecutionEnvironmentJob, ExecutionEnvironmentJobResult,
};

const EXPIRY_DIAGNOSTIC: &str = "native task operation deadline expired without positive proof";

/// Expire task-linked jobs while preserving the environment -> task -> binding
/// -> job lock order. The environment may already be held by the caller.
pub(crate) async fn expire_overdue_native_jobs(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: uuid::Uuid,
) -> anyhow::Result<()> {
    if crate::db::execution_environments::tasks::queries::lock_environment(
        tx,
        application_id,
        environment_id,
    )
    .await?
    .is_none()
    {
        return Ok(());
    }
    let expired: Vec<ExecutionEnvironmentJob> = sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environment_jobs/native_execution/find_expired_native_execution_jobs.sql",
        application_id,
        environment_id,
    )
    .fetch_all(&mut **tx)
    .await?;

    for candidate in expired {
        let Some(task_id) = candidate.native_task_id else {
            continue;
        };
        let (_, task, _binding) =
            match lock_context(tx, application_id, environment_id, task_id).await {
                Ok(context) => context,
                Err(_) => continue,
            };
        let Some(job) = lock_job(tx, application_id, environment_id, candidate.job_id)
            .await
            .map_err(|error| anyhow::anyhow!(error.code()))?
        else {
            continue;
        };
        if !matches!(job.state.as_str(), "pending" | "claimed")
            || job.deadline_at > chrono::Utc::now()
            || job.native_task_id != Some(task_id)
        {
            continue;
        }

        let (result, task_updated, environment_updated) = match job.job_kind.as_str() {
            "prepare_native_task" if task.prepare_job_id == Some(job.job_id) => {
                let result = serde_json::to_value(ExecutionEnvironmentJobResult {
                    outcome: "task_prepared_uncertain".to_string(),
                    observed_state: None,
                    confirmed: None,
                    preparation_quiescent: None,
                })?;
                let task_updated =
                    crate::db::execution_environments::tasks::prepare::mark_preparation_uncertain(
                        tx,
                        application_id,
                        environment_id,
                        task_id,
                    )
                    .await?;
                (result, task_updated.is_some(), true)
            }
            "start_native_task"
                if task.start_job_id == Some(job.job_id)
                    && matches!(task.state.as_str(), "starting" | "uncertain") =>
            {
                let result = serde_json::to_value(ExecutionEnvironmentJobResult {
                    outcome: "native_task_start_uncertain".to_string(),
                    observed_state: None,
                    confirmed: None,
                    preparation_quiescent: None,
                })?;
                let task_updated = sqlx::query_file_scalar!(
                    "src/sql/execution_environments/tasks/start/mark_native_task_uncertain.sql",
                    task_id,
                    application_id,
                    environment_id,
                    job.job_id,
                )
                .fetch_optional(&mut **tx)
                .await?;
                let environment_updated = sqlx::query_file_scalar!(
                    "src/sql/execution_environments/tasks/start/mark_environment_uncertain_for_task.sql",
                    environment_id,
                    application_id,
                    task_id,
                    job.target_generation,
                )
                .fetch_optional(&mut **tx)
                .await?;
                (
                    result,
                    task_updated.is_some(),
                    environment_updated.is_some(),
                )
            }
            "stop"
                if task.stop_job_id == Some(job.job_id)
                    && matches!(task.state.as_str(), "stopping" | "uncertain") =>
            {
                let result = serde_json::to_value(ExecutionEnvironmentJobResult {
                    outcome: "stop_unconfirmed".to_string(),
                    observed_state: None,
                    confirmed: Some(false),
                    preparation_quiescent: Some(false),
                })?;
                let task_updated = sqlx::query_file_scalar!(
                    "src/sql/execution_environments/tasks/stop/mark_native_task_uncertain.sql",
                    task_id,
                    application_id,
                    environment_id,
                    job.job_id,
                )
                .fetch_optional(&mut **tx)
                .await?;
                let environment_updated = sqlx::query_file_scalar!(
                    "src/sql/execution_environments/tasks/stop/mark_environment_uncertain_for_task.sql",
                    environment_id,
                    application_id,
                    task_id,
                    job.target_generation,
                )
                .fetch_optional(&mut **tx)
                .await?;
                (
                    result,
                    task_updated.is_some(),
                    environment_updated.is_some(),
                )
            }
            _ => continue,
        };
        if !task_updated || !environment_updated {
            anyhow::bail!("native task expiry lost its task or environment fence");
        }
        let expired = sqlx::query_file_scalar!(
            "src/sql/execution_environment_jobs/native_execution/expire_native_execution_job.sql",
            job.job_id,
            job.application_id,
            job.environment_id,
            task_id,
            result,
            EXPIRY_DIAGNOSTIC,
        )
        .fetch_optional(&mut **tx)
        .await?;
        if expired.is_none() {
            anyhow::bail!("native execution deadline changed while locked");
        }
    }
    Ok(())
}
