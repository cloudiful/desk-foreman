use sqlx::PgPool;
use uuid::Uuid;

use runner_protocol::{
    ExportNativeTaskResult, NativeTaskExportContent, validate_native_task_export_content,
};

use super::{
    identity::request_for_header,
    proof::{header_matches_task, lock_matching_stop_proof},
    queries,
    types::{NativeTaskExportDescriptor, NativeTaskExportError, TaskExportHeader, internal},
};
use crate::db::{
    execution_environment_jobs::ExecutionEnvironmentJob,
    execution_environments::tasks::{NativeTask, TaskEnvironment},
};

struct LockedExport {
    environment: TaskEnvironment,
    task: NativeTask,
    header: TaskExportHeader,
    job: ExecutionEnvironmentJob,
}

pub async fn find_native_task_export(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
) -> Result<NativeTaskExportDescriptor, NativeTaskExportError> {
    let mut tx = pool.begin().await.map_err(internal)?;
    let export = lock_export(&mut tx, application_id, environment_id, task_id).await?;
    validate_output_state(&export.header, &export.job)?;
    let descriptor = export.header.descriptor(&export.job.state);
    tx.commit().await.map_err(internal)?;
    Ok(descriptor)
}

pub async fn find_native_task_export_content(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
) -> Result<NativeTaskExportContent, NativeTaskExportError> {
    let mut tx = pool.begin().await.map_err(internal)?;
    let export = lock_export(&mut tx, application_id, environment_id, task_id).await?;
    validate_output_state(&export.header, &export.job)?;
    if export.job.state != "succeeded" {
        return Err(NativeTaskExportError::NotReady);
    }

    let request =
        request_for_header(&mut tx, &export.environment, &export.task, &export.header).await?;
    let files = queries::output_files(&mut tx, export.header.job_id)
        .await
        .map_err(internal)?
        .into_iter()
        .map(|file| runner_protocol::SourceSeedSnapshotFile::new(file.path, file.content))
        .collect::<Vec<_>>();
    let missing_paths = queries::missing_paths(&mut tx, export.header.job_id)
        .await
        .map_err(internal)?;
    let content = NativeTaskExportContent {
        task_id: export.task.task_id,
        stop_job_id: export.header.stop_job_id,
        selection_sha256: export.header.selection_sha256.clone(),
        output_manifest_sha256: export
            .header
            .output_manifest_sha256
            .clone()
            .ok_or(NativeTaskExportError::Internal)?,
        files,
        missing_paths,
    };
    let validated = validate_native_task_export_content(&request, &content)
        .map_err(|_| NativeTaskExportError::Internal)?;
    validate_result_summary(&export.header, &export.job, &validated)?;
    tx.commit().await.map_err(internal)?;
    Ok(validated.content)
}

async fn lock_export(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
) -> Result<LockedExport, NativeTaskExportError> {
    let (environment, task, binding) =
        queries::lock_context(tx, application_id, environment_id, task_id).await?;
    if binding.workspace_binding_id != task.workspace_binding_id {
        return Err(NativeTaskExportError::NotFound);
    }
    let header = queries::find_export_by_task(tx, task_id, environment_id, application_id)
        .await
        .map_err(internal)?
        .ok_or(NativeTaskExportError::NotFound)?;
    let job = queries::lock_job(tx, application_id, environment_id, header.job_id)
        .await?
        .ok_or(NativeTaskExportError::NotFound)?;
    let proof =
        lock_matching_stop_proof(tx, application_id, &environment, &task, header.stop_job_id)
            .await?;
    let _proof = proof;
    if !header_matches_task(&header, &environment, &task, header.stop_job_id)
        || job.job_kind != "export_native_task"
        || job.native_task_id != Some(task_id)
        || job.assigned_manager_id != header.assigned_manager_id
        || job.target_generation != header.target_generation
    {
        return Err(NativeTaskExportError::Internal);
    }
    Ok(LockedExport {
        environment,
        task,
        header,
        job,
    })
}

pub(super) fn validate_output_state(
    header: &TaskExportHeader,
    job: &ExecutionEnvironmentJob,
) -> Result<(), NativeTaskExportError> {
    let succeeded = job.state == "succeeded";
    if succeeded != header.output_manifest_sha256.is_some()
        || succeeded != header.completed_at.is_some()
        || succeeded != header.output_file_count.is_some()
        || succeeded != header.output_missing_count.is_some()
        || succeeded != header.output_total_bytes.is_some()
    {
        return Err(NativeTaskExportError::Internal);
    }
    if succeeded {
        let stored = job
            .result
            .as_ref()
            .and_then(|value| serde_json::from_value::<ExportNativeTaskResult>(value.clone()).ok())
            .ok_or(NativeTaskExportError::Internal)?;
        if stored.task_id != header.task_id
            || stored.stop_job_id != header.stop_job_id
            || stored.selection_sha256 != header.selection_sha256
            || stored.output_manifest_sha256
                != header.output_manifest_sha256.as_deref().unwrap_or_default()
            || Some(i32::try_from(stored.file_count).unwrap_or_default())
                != header.output_file_count
            || Some(i32::try_from(stored.missing_count).unwrap_or_default())
                != header.output_missing_count
            || Some(i64::from(stored.total_bytes)) != header.output_total_bytes
        {
            return Err(NativeTaskExportError::Internal);
        }
    }
    Ok(())
}

pub(super) fn validate_result_summary(
    header: &TaskExportHeader,
    job: &ExecutionEnvironmentJob,
    validated: &runner_protocol::ValidatedNativeTaskExport,
) -> Result<(), NativeTaskExportError> {
    let expected = ExportNativeTaskResult {
        task_id: header.task_id,
        stop_job_id: header.stop_job_id,
        selection_sha256: header.selection_sha256.clone(),
        output_manifest_sha256: validated.content.output_manifest_sha256.clone(),
        file_count: validated.file_count,
        missing_count: validated.missing_count,
        total_bytes: validated.total_bytes,
    };
    let stored = job
        .result
        .as_ref()
        .and_then(|value| serde_json::from_value::<ExportNativeTaskResult>(value.clone()).ok());
    if stored.as_ref() != Some(&expected)
        || header.output_file_count != i32::try_from(validated.file_count).ok()
        || header.output_missing_count != i32::try_from(validated.missing_count).ok()
        || header.output_total_bytes != Some(i64::from(validated.total_bytes))
    {
        return Err(NativeTaskExportError::Internal);
    }
    Ok(())
}
