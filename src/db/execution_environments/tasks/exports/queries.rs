use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::types::{TaskExportFile, TaskExportHeader};
use crate::db::{
    execution_environment_jobs::ExecutionEnvironmentJob,
    execution_environments::tasks::{NativeTask, TaskBinding, TaskEnvironment, TaskStopProof},
};

pub(super) async fn lock_context(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
) -> Result<(TaskEnvironment, NativeTask, TaskBinding), super::types::NativeTaskExportError> {
    super::super::native_execution::queries::lock_context(
        tx,
        application_id,
        environment_id,
        task_id,
    )
    .await
    .map_err(|error| match error {
        super::super::native_execution::types::TaskExecutionError::NotFound => {
            super::types::NativeTaskExportError::NotFound
        }
        _ => super::types::NativeTaskExportError::Internal,
    })
}

pub(super) async fn lock_job(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    job_id: Uuid,
) -> Result<Option<ExecutionEnvironmentJob>, super::types::NativeTaskExportError> {
    super::super::native_execution::queries::lock_job(tx, application_id, environment_id, job_id)
        .await
        .map_err(|error| match error {
            super::super::native_execution::types::TaskExecutionError::Internal => {
                super::types::NativeTaskExportError::Internal
            }
            _ => super::types::NativeTaskExportError::NotFound,
        })
}

pub(super) async fn lock_stop_proof(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    job_id: Uuid,
) -> anyhow::Result<Option<TaskStopProof>> {
    Ok(sqlx::query_file_as!(
        TaskStopProof,
        "src/sql/execution_environments/tasks/exports/lock_stop_proof.sql",
        job_id,
        application_id,
        environment_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

pub(super) async fn find_export_by_task(
    tx: &mut Transaction<'_, Postgres>,
    task_id: Uuid,
    environment_id: Uuid,
    application_id: i64,
) -> anyhow::Result<Option<TaskExportHeader>> {
    Ok(sqlx::query_file_as!(
        TaskExportHeader,
        "src/sql/execution_environments/tasks/exports/find_export_by_task.sql",
        task_id,
        environment_id,
        application_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

pub(super) async fn find_export_by_job(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    environment_id: Uuid,
    application_id: i64,
) -> anyhow::Result<Option<TaskExportHeader>> {
    Ok(sqlx::query_file_as!(
        TaskExportHeader,
        "src/sql/execution_environments/tasks/exports/find_export_by_job.sql",
        job_id,
        environment_id,
        application_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

pub(super) async fn insert_export(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    task_id: Uuid,
    environment_id: Uuid,
    application_id: i64,
    workspace_binding_id: i64,
    manager_id: i64,
    candidate_generation: i64,
    stop_job_id: Uuid,
    selection_sha256: &str,
    file_count: u32,
) -> anyhow::Result<Option<TaskExportHeader>> {
    Ok(sqlx::query_file_as!(
        TaskExportHeader,
        "src/sql/execution_environments/tasks/exports/insert_export.sql",
        job_id,
        task_id,
        environment_id,
        application_id,
        workspace_binding_id,
        manager_id,
        candidate_generation,
        stop_job_id,
        selection_sha256,
        i32::try_from(file_count)?,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

pub(super) async fn insert_selection_paths(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    paths: &[String],
) -> anyhow::Result<()> {
    sqlx::query_file!(
        "src/sql/execution_environments/tasks/exports/insert_selection_paths.sql",
        job_id,
        paths,
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) async fn selection_paths(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query_file!(
        "src/sql/execution_environments/tasks/exports/find_selection_paths.sql",
        job_id,
    )
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|row| row.path).collect())
}

pub(super) async fn output_files(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
) -> anyhow::Result<Vec<TaskExportFile>> {
    Ok(sqlx::query_file_as!(
        TaskExportFile,
        "src/sql/execution_environments/tasks/exports/find_output_files.sql",
        job_id,
    )
    .fetch_all(&mut **tx)
    .await?)
}

pub(super) async fn missing_paths(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query_file!(
        "src/sql/execution_environments/tasks/exports/find_missing_paths.sql",
        job_id,
    )
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows.into_iter().map(|row| row.path).collect())
}

pub(super) async fn insert_output(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    paths: &[String],
    contents: &[String],
) -> anyhow::Result<()> {
    sqlx::query_file!(
        "src/sql/execution_environments/tasks/exports/insert_output_files.sql",
        job_id,
        paths,
        contents,
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) async fn insert_missing(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    paths: &[String],
) -> anyhow::Result<()> {
    sqlx::query_file!(
        "src/sql/execution_environments/tasks/exports/insert_missing_paths.sql",
        job_id,
        paths,
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) async fn complete_header(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    output_manifest_sha256: &str,
    file_count: u32,
    missing_count: u32,
    total_bytes: u32,
) -> anyhow::Result<Option<TaskExportHeader>> {
    Ok(sqlx::query_file_as!(
        TaskExportHeader,
        "src/sql/execution_environments/tasks/exports/complete_export_header.sql",
        job_id,
        output_manifest_sha256,
        i32::try_from(file_count)?,
        i32::try_from(missing_count)?,
        i64::from(total_bytes),
    )
    .fetch_optional(&mut **tx)
    .await?)
}

pub(super) async fn claim_is_live(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
    manager_id: i64,
) -> anyhow::Result<bool> {
    Ok(sqlx::query_file_scalar!(
        "src/sql/execution_environments/tasks/exports/export_claim_is_live.sql",
        job_id,
        application_id,
        environment_id,
        task_id,
        manager_id,
    )
    .fetch_optional(&mut **tx)
    .await?
    .flatten()
    .unwrap_or(false))
}

pub(super) async fn complete_job(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
    manager_id: i64,
    result: serde_json::Value,
) -> anyhow::Result<Option<ExecutionEnvironmentJob>> {
    Ok(sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environments/tasks/exports/complete_export_job.sql",
        job_id,
        application_id,
        environment_id,
        task_id,
        manager_id,
        result,
    )
    .fetch_optional(&mut **tx)
    .await?)
}
