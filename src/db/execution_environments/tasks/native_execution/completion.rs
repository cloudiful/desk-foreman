use runner_protocol::RunnerJobResult;
use sqlx::PgPool;
use uuid::Uuid;

use super::completion_persistence::{expire_locked_job, mark_uncertain};
use super::completion_resolution::{resolve, runtime_identity};
use super::queries::{lock_context, lock_job};
use super::types::NativeJobCompletion;
use crate::db::execution_environment_jobs::ExecutionEnvironmentJob;

pub(crate) async fn complete_native_task_job(
    pool: &PgPool,
    manager_id: i64,
    job_id: Uuid,
    reported: &RunnerJobResult,
) -> anyhow::Result<NativeJobCompletion> {
    let Some(claimed) = sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environment_jobs/find_execution_environment_job_by_id.sql",
        job_id,
    )
    .fetch_optional(pool)
    .await?
    else {
        return Ok(NativeJobCompletion::NotFound);
    };
    let Some(task_id) = claimed.native_task_id else {
        return Ok(NativeJobCompletion::NotFound);
    };
    if !matches!(claimed.job_kind.as_str(), "start_native_task" | "stop") {
        return Ok(NativeJobCompletion::NotFound);
    }
    if claimed.assigned_manager_id != manager_id {
        return Ok(NativeJobCompletion::ForeignManager);
    }

    let mut tx = pool.begin().await?;
    let (environment, task, _binding) = match lock_context(
        &mut tx,
        claimed.application_id,
        claimed.environment_id,
        task_id,
    )
    .await
    {
        Ok(context) => context,
        Err(error) => {
            tx.rollback().await?;
            return Ok(
                if matches!(error, super::types::TaskExecutionError::NotFound) {
                    NativeJobCompletion::NotFound
                } else {
                    NativeJobCompletion::Inconsistent("native task context is no longer valid")
                },
            );
        }
    };
    let Some(job) = lock_job(
        &mut tx,
        claimed.application_id,
        claimed.environment_id,
        job_id,
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.code()))?
    else {
        tx.rollback().await?;
        return Ok(NativeJobCompletion::NotFound);
    };
    if job.assigned_manager_id != manager_id {
        tx.rollback().await?;
        return Ok(NativeJobCompletion::ForeignManager);
    }
    if job.native_task_id != Some(task_id)
        || task.assigned_manager_id != manager_id
        || environment.assigned_manager_id != Some(manager_id)
    {
        tx.rollback().await?;
        return Ok(NativeJobCompletion::Inconsistent(
            "native execution manager or task link does not match",
        ));
    }

    let identity = match runtime_identity(
        &mut tx,
        claimed.application_id,
        claimed.environment_id,
        &task,
        job.job_kind == "start_native_task",
    )
    .await
    {
        Ok(Some(identity)) => identity,
        Ok(None) => {
            tx.rollback().await?;
            return Ok(NativeJobCompletion::Inconsistent(
                "native task has no succeeded matching preparation proof",
            ));
        }
        Err(error) => return Err(error),
    };
    let resolution = match resolve(&job, reported, &identity) {
        Ok(resolution) => resolution,
        Err(reason) => {
            tx.rollback().await?;
            return Ok(NativeJobCompletion::Inconsistent(reason));
        }
    };

    if matches!(
        job.state.as_str(),
        "succeeded" | "uncertain" | "failed" | "superseded"
    ) {
        let expected = serde_json::to_value(&resolution.result)?;
        let replay = job.state == resolution.state && job.result.as_ref() == Some(&expected);
        tx.rollback().await?;
        return Ok(if replay {
            NativeJobCompletion::AlreadyTerminal
        } else {
            NativeJobCompletion::NotCompletable
        });
    }
    if job.state != "claimed" {
        tx.rollback().await?;
        return Ok(NativeJobCompletion::NotCompletable);
    }

    if job.deadline_at <= chrono::Utc::now() {
        expire_locked_job(&mut tx, &environment, &task, &job).await?;
        tx.commit().await?;
        return Ok(NativeJobCompletion::NotCompletable);
    }

    let start_current = job.job_kind == "start_native_task"
        && task.state == "starting"
        && task.start_job_id == Some(job_id)
        && task.target_generation == job.target_generation
        && task.container_generation == Some(job.target_generation)
        && environment.active_native_task_id == Some(task_id)
        && environment.state == "starting"
        && environment.generation == job.target_generation
        && environment.container_generation == Some(job.target_generation);
    let stop_current = job.job_kind == "stop"
        && task.state == "stopping"
        && task.stop_job_id == Some(job_id)
        && task.target_generation == job.target_generation
        && task.container_generation == Some(job.target_generation)
        && environment.active_native_task_id == Some(task_id)
        && environment.state == "stopping"
        && environment.generation > job.target_generation
        && environment.container_generation == Some(job.target_generation);
    if !(start_current || stop_current) {
        tx.rollback().await?;
        return Ok(NativeJobCompletion::NotCompletable);
    }

    if resolution.successful_start {
        let task_updated = sqlx::query_file_scalar!(
            "src/sql/execution_environments/tasks/start/mark_native_task_running.sql",
            task_id,
            claimed.application_id,
            claimed.environment_id,
            job_id,
            job.target_generation,
        )
        .fetch_optional(&mut *tx)
        .await?;
        let environment_updated = sqlx::query_file_scalar!(
            "src/sql/execution_environments/tasks/start/mark_environment_ready_for_task.sql",
            claimed.environment_id,
            claimed.application_id,
            task_id,
            job.target_generation,
        )
        .fetch_optional(&mut *tx)
        .await?;
        if task_updated.is_none() || environment_updated.is_none() {
            tx.rollback().await?;
            return Ok(NativeJobCompletion::NotCompletable);
        }
    } else if resolution.successful_stop {
        let task_updated = sqlx::query_file_scalar!(
            "src/sql/execution_environments/tasks/stop/mark_native_task_stop_confirmed.sql",
            task_id,
            claimed.application_id,
            claimed.environment_id,
            job_id,
            job.target_generation,
        )
        .fetch_optional(&mut *tx)
        .await?;
        let environment_updated = sqlx::query_file_scalar!(
            "src/sql/execution_environments/tasks/stop/mark_environment_stop_confirmed.sql",
            claimed.environment_id,
            claimed.application_id,
            task_id,
            job_id,
            job.target_generation,
        )
        .fetch_optional(&mut *tx)
        .await?;
        if task_updated.is_none() || environment_updated.is_none() {
            tx.rollback().await?;
            return Ok(NativeJobCompletion::NotCompletable);
        }
        let superseded = serde_json::json!({"outcome": "superseded_by_native_task_stop"});
        let _: Vec<Uuid> = sqlx::query_file_scalar!(
            "src/sql/execution_environments/tasks/stop/supersede_inflight_task_jobs.sql",
            task_id,
            superseded,
            job_id,
        )
        .fetch_all(&mut *tx)
        .await?;
    } else {
        mark_uncertain(&mut tx, &environment, &task, &job).await?;
    }

    let result_value = serde_json::to_value(&resolution.result)?;
    let completed = if job.job_kind == "start_native_task" {
        sqlx::query_file_scalar!(
            "src/sql/execution_environments/tasks/start/complete_native_start_job.sql",
            job_id,
            claimed.application_id,
            claimed.environment_id,
            result_value,
            task_id,
            manager_id,
            resolution.state,
            resolution.error_summary,
        )
        .fetch_optional(&mut *tx)
        .await?
    } else {
        sqlx::query_file_scalar!(
            "src/sql/execution_environments/tasks/stop/complete_native_stop_job.sql",
            job_id,
            claimed.application_id,
            claimed.environment_id,
            resolution.state,
            result_value,
            resolution.error_summary,
            task_id,
            manager_id,
        )
        .fetch_optional(&mut *tx)
        .await?
    };
    if completed.is_none() {
        tx.rollback().await?;
        return Ok(NativeJobCompletion::NotCompletable);
    }
    tx.commit().await?;
    Ok(NativeJobCompletion::Completed)
}
