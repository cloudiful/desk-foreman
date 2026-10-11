use super::{
    proof::candidate,
    queries,
    types::{NativeTaskExportError, TaskExportHeader},
};
use crate::db::execution_environments::tasks::TaskEnvironment;
use runner_protocol::{ExportNativeTaskRequest, NativeTaskRuntimeIdentity, RunnerOwner};

pub(super) async fn request_for_header(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    environment: &TaskEnvironment,
    task: &crate::db::execution_environments::tasks::NativeTask,
    header: &TaskExportHeader,
) -> Result<ExportNativeTaskRequest, NativeTaskExportError> {
    let candidate_generation = candidate(task)?;
    if header.task_id != task.task_id
        || header.environment_id != task.environment_id
        || header.application_id != task.application_id
        || header.workspace_binding_id != task.workspace_binding_id
        || header.assigned_manager_id != task.assigned_manager_id
        || header.target_generation != candidate_generation
        || task.stop_job_id != Some(header.stop_job_id)
        || environment.assigned_manager_id != Some(task.assigned_manager_id)
    {
        return Err(NativeTaskExportError::Conflict);
    }

    let paths = queries::selection_paths(tx, header.job_id)
        .await
        .map_err(super::types::internal)?;
    let selection = runner_protocol::validate_native_task_export_selection(&paths)
        .map_err(|_| NativeTaskExportError::Internal)?;
    if selection.selection_sha256 != header.selection_sha256
        || i32::try_from(selection.file_count).ok() != Some(header.selection_file_count)
    {
        return Err(NativeTaskExportError::Internal);
    }
    let prepare_job_id = task.prepare_job_id.ok_or(NativeTaskExportError::Conflict)?;
    let preparation =
        crate::db::execution_environments::tasks::native_execution::queries::find_job(
            tx,
            task.application_id,
            task.environment_id,
            prepare_job_id,
        )
        .await
        .map_err(|error| super::types::internal(error.code()))?
        .ok_or(NativeTaskExportError::Conflict)?;
    if preparation.job_kind != "prepare_native_task"
        || preparation.native_task_id != Some(task.task_id)
        || preparation.assigned_manager_id != task.assigned_manager_id
        || preparation.target_generation != task.target_generation
        || !matches!(preparation.state.as_str(), "succeeded" | "uncertain")
    {
        return Err(NativeTaskExportError::Conflict);
    }
    let Some((manifest_sha256, _, _)) =
        crate::db::execution_environments::tasks::native_execution::queries::seed_identity(
            tx,
            task.task_id,
        )
        .await
        .map_err(|error| super::types::internal(error.code()))?
    else {
        return Err(NativeTaskExportError::Conflict);
    };
    Ok(ExportNativeTaskRequest {
        owner: RunnerOwner::ExecutionEnvironment {
            environment_id: environment.environment_id.to_string(),
            generation: candidate_generation,
        },
        task: NativeTaskRuntimeIdentity {
            task_id: task.task_id,
            workspace_binding_id: task.workspace_binding_id,
            manifest_sha256,
            preparation_generation: preparation.target_generation,
        },
        stop_job_id: header.stop_job_id,
        selection_sha256: selection.selection_sha256,
        paths: selection.paths,
    })
}
