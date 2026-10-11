use uuid::Uuid;

use super::types::{NativeTaskExportError, TaskExportHeader};
use crate::db::execution_environments::tasks::{NativeTask, TaskEnvironment, TaskStopProof};

pub(super) fn candidate(task: &NativeTask) -> Result<i64, NativeTaskExportError> {
    let Some(candidate) = task.container_generation else {
        return Err(NativeTaskExportError::Conflict);
    };
    if !matches!(task.state.as_str(), "stopping" | "released")
        || task.target_generation != candidate
    {
        return Err(NativeTaskExportError::Conflict);
    }
    Ok(candidate)
}

pub(super) async fn lock_matching_stop_proof(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    application_id: i64,
    environment: &TaskEnvironment,
    task: &NativeTask,
    stop_job_id: Uuid,
) -> Result<TaskStopProof, NativeTaskExportError> {
    let Some(proof) = super::queries::lock_stop_proof(
        tx,
        application_id,
        environment.environment_id,
        stop_job_id,
    )
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "native task export stop proof read failed");
        NativeTaskExportError::Internal
    })?
    else {
        return Err(NativeTaskExportError::Conflict);
    };
    if !proof_matches_task(&proof, environment, task, stop_job_id) {
        return Err(NativeTaskExportError::Conflict);
    }
    Ok(proof)
}

pub(super) fn proof_matches_task(
    proof: &TaskStopProof,
    environment: &TaskEnvironment,
    task: &NativeTask,
    stop_job_id: Uuid,
) -> bool {
    let Ok(candidate) = candidate(task) else {
        return false;
    };
    let result = proof.result.as_ref().and_then(|value| {
        serde_json::from_value::<
                crate::db::execution_environment_jobs::ExecutionEnvironmentJobResult,
            >(value.clone())
            .ok()
    });
    let quiescent = result
        .as_ref()
        .is_some_and(|value| value.preparation_quiescent == Some(true));
    proof.job_id == stop_job_id
        && task.stop_job_id == Some(stop_job_id)
        && proof.environment_id == task.environment_id
        && proof.application_id == task.application_id
        && proof.environment_id == environment.environment_id
        && proof.application_id == environment.application_id
        && proof.job_kind == "stop"
        && proof.state == "succeeded"
        && proof.native_task_id == Some(task.task_id)
        && proof.assigned_manager_id == task.assigned_manager_id
        && environment.assigned_manager_id == Some(task.assigned_manager_id)
        && proof.target_generation == candidate
        && quiescent
        && crate::db::execution_environments::lifecycle::confirmed_stop_state(proof.result.as_ref())
            .is_some()
}

pub(super) fn header_matches_task(
    header: &TaskExportHeader,
    environment: &TaskEnvironment,
    task: &NativeTask,
    stop_job_id: Uuid,
) -> bool {
    candidate(task).is_ok_and(|candidate| {
        header.task_id == task.task_id
            && header.environment_id == task.environment_id
            && header.application_id == task.application_id
            && header.environment_id == environment.environment_id
            && header.application_id == environment.application_id
            && header.workspace_binding_id == task.workspace_binding_id
            && header.assigned_manager_id == task.assigned_manager_id
            && environment.assigned_manager_id == Some(task.assigned_manager_id)
            && header.target_generation == candidate
            && header.stop_job_id == stop_job_id
            && task.stop_job_id == Some(stop_job_id)
            && header.selection_file_count > 0
    })
}
