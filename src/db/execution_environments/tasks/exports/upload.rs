use runner_protocol::{
    ExportNativeTaskResult, ExportNativeTaskUpload, NativeTaskExportContent,
    validate_native_task_export_content,
};
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    identity::request_for_header,
    proof::{header_matches_task, lock_matching_stop_proof},
    queries,
    read::{validate_output_state, validate_result_summary},
    types::{NativeTaskExportError, TaskExportHeader, internal},
};
use crate::db::execution_environment_jobs::find_execution_environment_job_by_id;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeTaskExportUploadOutcome {
    Uploaded,
    AlreadyUploaded,
}

pub async fn upload_native_task_export(
    pool: &PgPool,
    manager_id: i64,
    job_id: Uuid,
    upload: ExportNativeTaskUpload,
) -> Result<NativeTaskExportUploadOutcome, NativeTaskExportError> {
    let validated = validate_native_task_export_content(&upload.request, &upload.content)
        .map_err(NativeTaskExportError::InvalidContent)?;
    let Some(observed) = find_execution_environment_job_by_id(pool, job_id)
        .await
        .map_err(internal)?
    else {
        return Err(NativeTaskExportError::NotFound);
    };
    let Some(task_id) = observed.native_task_id else {
        return Err(NativeTaskExportError::NotFound);
    };
    if observed.job_kind != "export_native_task" || observed.assigned_manager_id != manager_id {
        return Err(NativeTaskExportError::NotFound);
    }

    let mut tx = pool.begin().await.map_err(internal)?;
    let (environment, task, binding) = queries::lock_context(
        &mut tx,
        observed.application_id,
        observed.environment_id,
        task_id,
    )
    .await?;
    if binding.workspace_binding_id != task.workspace_binding_id {
        return Err(NativeTaskExportError::NotFound);
    }
    let job = queries::lock_job(
        &mut tx,
        observed.application_id,
        observed.environment_id,
        job_id,
    )
    .await?
    .ok_or(NativeTaskExportError::NotFound)?;
    let header =
        queries::find_export_by_job(&mut tx, job_id, job.environment_id, job.application_id)
            .await
            .map_err(internal)?
            .ok_or(NativeTaskExportError::NotFound)?;
    let _proof = lock_matching_stop_proof(
        &mut tx,
        job.application_id,
        &environment,
        &task,
        header.stop_job_id,
    )
    .await?;
    if !header_matches_task(&header, &environment, &task, header.stop_job_id)
        || job.job_kind != "export_native_task"
        || job.native_task_id != Some(task_id)
        || job.assigned_manager_id != manager_id
        || job.target_generation != header.target_generation
        || header.job_id != job_id
    {
        return Err(NativeTaskExportError::NotFound);
    }
    let expected_request = request_for_header(&mut tx, &environment, &task, &header).await?;
    if upload.request != expected_request {
        return Err(NativeTaskExportError::Conflict);
    }

    if job.state == "succeeded" {
        validate_output_state(&header, &job)?;
        let stored = stored_content(&mut tx, &header).await?;
        let stored = validate_native_task_export_content(&expected_request, &stored)
            .map_err(|_| NativeTaskExportError::Internal)?;
        validate_result_summary(&header, &job, &stored)?;
        if stored.content != validated.content {
            return Err(NativeTaskExportError::Conflict);
        }
        tx.commit().await.map_err(internal)?;
        return Ok(NativeTaskExportUploadOutcome::AlreadyUploaded);
    }
    if job.state != "claimed"
        || !queries::claim_is_live(
            &mut tx,
            job.job_id,
            job.application_id,
            job.environment_id,
            task_id,
            manager_id,
        )
        .await
        .map_err(internal)?
    {
        return Err(NativeTaskExportError::NotCompletable);
    }

    let file_paths = validated
        .content
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    let contents = validated
        .content
        .files
        .iter()
        .map(|file| file.content.clone())
        .collect::<Vec<_>>();
    queries::insert_output(&mut tx, job_id, &file_paths, &contents)
        .await
        .map_err(internal)?;
    queries::insert_missing(&mut tx, job_id, &validated.content.missing_paths)
        .await
        .map_err(internal)?;
    let completed_header = queries::complete_header(
        &mut tx,
        job_id,
        &validated.content.output_manifest_sha256,
        validated.file_count,
        validated.missing_count,
        validated.total_bytes,
    )
    .await
    .map_err(internal)?
    .ok_or(NativeTaskExportError::NotCompletable)?;
    let result = ExportNativeTaskResult {
        task_id,
        stop_job_id: header.stop_job_id,
        selection_sha256: header.selection_sha256.clone(),
        output_manifest_sha256: validated.content.output_manifest_sha256.clone(),
        file_count: validated.file_count,
        missing_count: validated.missing_count,
        total_bytes: validated.total_bytes,
    };
    let result = serde_json::to_value(result).map_err(internal)?;
    let completed = queries::complete_job(
        &mut tx,
        job_id,
        job.application_id,
        job.environment_id,
        task_id,
        manager_id,
        result,
    )
    .await
    .map_err(internal)?
    .ok_or(NativeTaskExportError::NotCompletable)?;
    if completed.state != "succeeded"
        || completed_header.output_manifest_sha256.as_deref()
            != Some(validated.content.output_manifest_sha256.as_str())
    {
        return Err(NativeTaskExportError::Internal);
    }
    tx.commit().await.map_err(internal)?;
    Ok(NativeTaskExportUploadOutcome::Uploaded)
}

async fn stored_content(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    header: &TaskExportHeader,
) -> Result<NativeTaskExportContent, NativeTaskExportError> {
    let files = queries::output_files(tx, header.job_id)
        .await
        .map_err(internal)?
        .into_iter()
        .map(|file| runner_protocol::SourceSeedSnapshotFile::new(file.path, file.content))
        .collect();
    let missing_paths = queries::missing_paths(tx, header.job_id)
        .await
        .map_err(internal)?;
    Ok(NativeTaskExportContent {
        task_id: header.task_id,
        stop_job_id: header.stop_job_id,
        selection_sha256: header.selection_sha256.clone(),
        output_manifest_sha256: header
            .output_manifest_sha256
            .clone()
            .ok_or(NativeTaskExportError::Internal)?,
        files,
        missing_paths,
    })
}
