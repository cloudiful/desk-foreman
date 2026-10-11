use sqlx::PgPool;

use super::{
    proof::{header_matches_task, lock_matching_stop_proof},
    queries,
    types::NativeTaskExportError,
};
use crate::db::execution_environment_jobs::{
    DurableJobCompletion, ExecutionEnvironmentJob, ExecutionEnvironmentJobResult,
    job_state_is_terminal,
};

const FAILURE_DIAGNOSTIC: &str = "assigned manager failed native task export";

pub(super) async fn complete_export_failure(
    pool: &PgPool,
    manager_id: i64,
    observed: &ExecutionEnvironmentJob,
) -> anyhow::Result<DurableJobCompletion> {
    let Some(task_id) = observed.native_task_id else {
        return Ok(DurableJobCompletion::NotFound);
    };
    let mut tx = pool.begin().await?;
    let (environment, task, binding) = match queries::lock_context(
        &mut tx,
        observed.application_id,
        observed.environment_id,
        task_id,
    )
    .await
    {
        Ok(context) => context,
        Err(NativeTaskExportError::Internal) => {
            anyhow::bail!("native task export failure context could not be read")
        }
        Err(_) => return Ok(DurableJobCompletion::NotFound),
    };
    let Some(job) = queries::lock_job(
        &mut tx,
        observed.application_id,
        observed.environment_id,
        observed.job_id,
    )
    .await
    .map_err(|error| anyhow::anyhow!(error.code()))?
    else {
        return Ok(DurableJobCompletion::NotFound);
    };
    let Some(header) =
        queries::find_export_by_job(&mut tx, job.job_id, job.environment_id, job.application_id)
            .await?
    else {
        return Ok(DurableJobCompletion::NotFound);
    };
    if job.assigned_manager_id != manager_id {
        return Ok(DurableJobCompletion::ForeignManager);
    }
    match lock_matching_stop_proof(
        &mut tx,
        job.application_id,
        &environment,
        &task,
        header.stop_job_id,
    )
    .await
    {
        Ok(_) => {}
        Err(NativeTaskExportError::Internal) => {
            anyhow::bail!("native task export failure proof could not be read")
        }
        Err(_) => return Ok(DurableJobCompletion::NotCompletable),
    }
    if binding.workspace_binding_id != task.workspace_binding_id
        || !header_matches_task(&header, &environment, &task, header.stop_job_id)
        || job.job_kind != "export_native_task"
        || job.native_task_id != Some(task_id)
        || job.target_generation != header.target_generation
        || header.output_manifest_sha256.is_some()
    {
        return Ok(DurableJobCompletion::TerminalConflict);
    }
    let result = serde_json::to_value(ExecutionEnvironmentJobResult {
        outcome: "native_task_export_failed".to_string(),
        observed_state: None,
        confirmed: None,
        preparation_quiescent: None,
    })?;
    if job.state == "failed" {
        return Ok(
            if job.result.as_ref() == Some(&result)
                && job.error_summary.as_deref() == Some(FAILURE_DIAGNOSTIC)
            {
                DurableJobCompletion::AlreadyTerminal
            } else {
                DurableJobCompletion::TerminalConflict
            },
        );
    }
    if job_state_is_terminal(&job.state) {
        return Ok(DurableJobCompletion::TerminalConflict);
    }
    let completed = sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environments/tasks/exports/fail_export_job.sql",
        job.job_id,
        job.application_id,
        job.environment_id,
        task_id,
        manager_id,
        result,
    )
    .fetch_optional(&mut *tx)
    .await?;
    if completed.is_none() {
        return Ok(DurableJobCompletion::NotCompletable);
    }
    tx.commit().await?;
    Ok(DurableJobCompletion::Completed)
}
