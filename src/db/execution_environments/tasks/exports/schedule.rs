use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use runner_protocol::validate_native_task_export_selection;

use super::{
    proof::{candidate, header_matches_task, lock_matching_stop_proof, proof_matches_task},
    queries,
    types::{NativeTaskExportDescriptor, NativeTaskExportError, internal},
};
use crate::db::{
    execution_environment_jobs::job_token_is_valid, execution_environments::tasks::native_execution,
};

const EXPORT_JOB_DEADLINE_SECS: i64 = 300;

pub async fn schedule_native_task_export(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
    operation_token: &str,
    stop_job_id: Uuid,
    paths: &[String],
) -> Result<NativeTaskExportDescriptor, NativeTaskExportError> {
    if !job_token_is_valid(operation_token) {
        return Err(NativeTaskExportError::InvalidToken);
    }
    let selection = validate_native_task_export_selection(paths)
        .map_err(NativeTaskExportError::InvalidSelection)?;
    let mut tx = pool.begin().await.map_err(internal)?;
    let (environment, task, binding) =
        queries::lock_context(&mut tx, application_id, environment_id, task_id).await?;
    if binding.workspace_binding_id != task.workspace_binding_id
        || binding.application_id != application_id
    {
        return Err(NativeTaskExportError::NotFound);
    }
    let candidate_generation = candidate(&task)?;
    if task.stop_job_id != Some(stop_job_id)
        || environment.assigned_manager_id != Some(task.assigned_manager_id)
    {
        return Err(NativeTaskExportError::Conflict);
    }

    if let Some(header) =
        queries::find_export_by_task(&mut tx, task_id, environment_id, application_id)
            .await
            .map_err(internal)?
    {
        let job = queries::lock_job(&mut tx, application_id, environment_id, header.job_id)
            .await?
            .ok_or(NativeTaskExportError::Conflict)?;
        let proof = lock_matching_stop_proof(
            &mut tx,
            application_id,
            &environment,
            &task,
            header.stop_job_id,
        )
        .await?;
        if !header_matches_task(&header, &environment, &task, stop_job_id)
            || !proof_matches_task(&proof, &environment, &task, stop_job_id)
            || job.job_kind != "export_native_task"
            || job.native_task_id != Some(task_id)
            || job.target_generation != candidate_generation
            || job.assigned_manager_id != task.assigned_manager_id
            || job.token != operation_token
            || header.selection_sha256 != selection.selection_sha256
        {
            return Err(NativeTaskExportError::Conflict);
        }
        let paths = queries::selection_paths(&mut tx, header.job_id)
            .await
            .map_err(internal)?;
        let stored_selection = validate_native_task_export_selection(&paths)
            .map_err(|_| NativeTaskExportError::Internal)?;
        if stored_selection != selection
            || i32::try_from(stored_selection.file_count).ok() != Some(header.selection_file_count)
        {
            return Err(NativeTaskExportError::Conflict);
        }
        let descriptor = header.descriptor(&job.state);
        tx.commit().await.map_err(internal)?;
        return Ok(descriptor);
    }

    if native_execution::queries::find_job_by_token(
        &mut tx,
        application_id,
        environment_id,
        operation_token,
    )
    .await
    .map_err(|error| internal(error.code()))?
    .is_some()
    {
        return Err(NativeTaskExportError::Conflict);
    }

    let proof =
        lock_matching_stop_proof(&mut tx, application_id, &environment, &task, stop_job_id).await?;
    if !proof_matches_task(&proof, &environment, &task, stop_job_id) {
        return Err(NativeTaskExportError::Conflict);
    }
    if !native_execution::queries::manager_is_usable(&mut tx, task.assigned_manager_id)
        .await
        .map_err(|error| internal(error.code()))?
    {
        return Err(NativeTaskExportError::ManagerUnavailable);
    }

    let job_id = Uuid::new_v4();
    let deadline_at = Utc::now() + chrono::Duration::seconds(EXPORT_JOB_DEADLINE_SECS);
    let job = native_execution::queries::insert_job(
        &mut tx,
        job_id,
        operation_token,
        environment_id,
        application_id,
        task.assigned_manager_id,
        "export_native_task",
        candidate_generation,
        task_id,
        None,
        deadline_at,
    )
    .await
    .map_err(|error| internal(error.code()))?
    .ok_or(NativeTaskExportError::Conflict)?;
    let header = queries::insert_export(
        &mut tx,
        job_id,
        task_id,
        environment_id,
        application_id,
        task.workspace_binding_id,
        task.assigned_manager_id,
        candidate_generation,
        stop_job_id,
        &selection.selection_sha256,
        selection.file_count,
    )
    .await
    .map_err(internal)?
    .ok_or(NativeTaskExportError::Conflict)?;
    queries::insert_selection_paths(&mut tx, job_id, &selection.paths)
        .await
        .map_err(internal)?;
    let descriptor = header.descriptor(&job.state);
    tx.commit().await.map_err(internal)?;
    Ok(descriptor)
}
