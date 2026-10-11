use runner_protocol::{ExportNativeTaskResult, RunnerJobResult};
use sqlx::PgPool;

use super::failure::complete_export_failure;
use crate::db::execution_environment_jobs::{
    DurableJobCompletion, ExecutionEnvironmentJob, find_execution_environment_job_by_id,
    job_state_is_terminal,
};

pub(crate) async fn complete_export_result(
    pool: &PgPool,
    manager_id: i64,
    row: &ExecutionEnvironmentJob,
    reported: &RunnerJobResult,
) -> anyhow::Result<DurableJobCompletion> {
    if !reported.ok {
        if !reported
            .error
            .as_deref()
            .is_some_and(|error| !error.trim().is_empty())
        {
            return Ok(DurableJobCompletion::Inconsistent(
                "failed export result carried no diagnostic",
            ));
        }
        return complete_export_failure(pool, manager_id, row).await;
    }

    let Some(value) = reported.result.clone() else {
        return Ok(DurableJobCompletion::Inconsistent(
            "successful export result carried no summary",
        ));
    };
    let Ok(summary) = serde_json::from_value::<ExportNativeTaskResult>(value.clone()) else {
        return Ok(DurableJobCompletion::Inconsistent(
            "export result summary could not be decoded",
        ));
    };
    let Some(current) = find_execution_environment_job_by_id(pool, row.job_id).await? else {
        return Ok(DurableJobCompletion::NotFound);
    };
    if current.assigned_manager_id != manager_id {
        return Ok(DurableJobCompletion::ForeignManager);
    }
    if current.state != "succeeded" {
        return Ok(if job_state_is_terminal(&current.state) {
            DurableJobCompletion::TerminalConflict
        } else {
            DurableJobCompletion::NotCompletable
        });
    }
    if current.result.as_ref() != Some(&value) {
        return Ok(DurableJobCompletion::TerminalConflict);
    }
    let Some(task_id) = current.native_task_id else {
        return Ok(DurableJobCompletion::Inconsistent(
            "successful export job has no task link",
        ));
    };
    if summary.task_id != task_id {
        return Ok(DurableJobCompletion::TerminalConflict);
    }
    match crate::db::execution_environments::tasks::exports::find_native_task_export_content(
        pool,
        current.application_id,
        current.environment_id,
        task_id,
    )
    .await
    {
        Ok(_) => Ok(DurableJobCompletion::AlreadyTerminal),
        Err(crate::db::execution_environments::tasks::exports::NativeTaskExportError::Internal) => {
            anyhow::bail!("stored native task export is inconsistent")
        }
        Err(_) => Ok(DurableJobCompletion::NotCompletable),
    }
}
