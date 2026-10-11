use sqlx::PgPool;
use uuid::Uuid;

use super::native_execution::{
    NativeTaskOperationDescriptor, TaskExecutionError, TaskExecutionRequest,
};
use super::native_execution::{queries, types};
use crate::db::execution_environment_jobs::{ExecutionEnvironmentJob, job_token_is_valid};

const JOB_DEADLINE_SECS: i64 = 300;

pub(super) async fn start(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
    request: TaskExecutionRequest,
) -> Result<NativeTaskOperationDescriptor, TaskExecutionError> {
    if !job_token_is_valid(&request.operation_token) {
        return Err(TaskExecutionError::InvalidToken);
    }

    let mut tx = pool.begin().await.map_err(types::internal)?;
    super::native_execution::expire_overdue_native_jobs(&mut tx, application_id, environment_id)
        .await
        .map_err(types::internal)?;
    let (environment, task, binding) =
        queries::lock_context(&mut tx, application_id, environment_id, task_id).await?;

    if let Some(start_job_id) = task.start_job_id {
        let existing = queries::lock_job(&mut tx, application_id, environment_id, start_job_id)
            .await?
            .ok_or(TaskExecutionError::Conflict)?;
        if existing.token != request.operation_token
            || !is_start_for_task(&existing, task_id)
            || existing.assigned_manager_id != task.assigned_manager_id
            || existing.target_generation != task.target_generation
            || environment.assigned_manager_id != Some(existing.assigned_manager_id)
            || request.expected_generation != existing.target_generation
        {
            return Err(TaskExecutionError::TokenConflict);
        }
        let descriptor = NativeTaskOperationDescriptor::new(&environment, &task, &existing);
        tx.commit().await.map_err(types::internal)?;
        return Ok(descriptor);
    }

    // A token already attached to another operation is an opaque conflict, not
    // an authorization or database error.
    if queries::find_job_by_token(
        &mut tx,
        application_id,
        environment_id,
        &request.operation_token,
    )
    .await?
    .is_some()
    {
        return Err(TaskExecutionError::TokenConflict);
    }

    if request.expected_generation != environment.generation
        || request.expected_generation != task.target_generation
    {
        return Err(TaskExecutionError::StaleGeneration);
    }
    if !binding.is_active_code_project()
        || binding.lifecycle_blocks_admission()
        || binding.has_legacy_write_owner()
        || environment.active_native_task_id != Some(task_id)
        || environment.assigned_manager_id != Some(task.assigned_manager_id)
        || task.state != "prepared"
        || task.container_generation.is_some()
        || task.subject_external_user_id != environment.external_user_id
    {
        return Err(TaskExecutionError::Conflict);
    }
    validate_preparation(&mut tx, application_id, environment_id, &task).await?;
    validate_environment_candidate(
        &mut tx,
        application_id,
        environment_id,
        &environment,
        task.assigned_manager_id,
    )
    .await?;
    validate_no_legacy_blockers(&mut tx, application_id, environment_id, task_id).await?;
    if !queries::manager_is_usable(&mut tx, task.assigned_manager_id).await? {
        return Err(TaskExecutionError::ManagerUnavailable);
    }

    let job_id = Uuid::new_v4();
    let job = queries::insert_job(
        &mut tx,
        job_id,
        &request.operation_token,
        environment_id,
        application_id,
        task.assigned_manager_id,
        "start_native_task",
        task.target_generation,
        task_id,
        None,
        types::deadline_after(JOB_DEADLINE_SECS),
    )
    .await?;
    let Some(job) = job else {
        // A token or active-task unique conflict may be owned by a different
        // task or operation. Only an exact task-local replay is observable.
        let replay = queries::find_job_by_token(
            &mut tx,
            application_id,
            environment_id,
            &request.operation_token,
        )
        .await?;
        let Some(replay) = replay.filter(|row| is_start_for_task(row, task_id)) else {
            return Err(TaskExecutionError::TokenConflict);
        };
        let descriptor = NativeTaskOperationDescriptor::new(&environment, &task, &replay);
        tx.commit().await.map_err(types::internal)?;
        return Ok(descriptor);
    };

    let starting = sqlx::query_file_as!(
        crate::db::execution_environments::NativeTask,
        "src/sql/execution_environments/tasks/start/mark_task_starting.sql",
        task_id,
        application_id,
        environment_id,
        task.target_generation,
        job.job_id,
        task.prepare_job_id,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(types::internal)?
    .ok_or(TaskExecutionError::Conflict)?;

    let environment_started = sqlx::query_file_scalar!(
        "src/sql/execution_environments/tasks/start/mark_environment_starting.sql",
        environment_id,
        application_id,
        task_id,
        task.target_generation,
        environment.container_generation,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(types::internal)?;
    if environment_started.is_none() {
        return Err(TaskExecutionError::Conflict);
    }

    let updated_environment =
        super::queries::lock_environment(&mut tx, application_id, environment_id)
            .await
            .map_err(types::internal)?
            .ok_or(TaskExecutionError::NotFound)?;
    let descriptor = NativeTaskOperationDescriptor::new(&updated_environment, &starting, &job);
    tx.commit().await.map_err(types::internal)?;
    Ok(descriptor)
}

fn is_start_for_task(job: &ExecutionEnvironmentJob, task_id: Uuid) -> bool {
    job.job_kind == "start_native_task" && job.native_task_id == Some(task_id)
}

async fn validate_preparation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    application_id: i64,
    environment_id: Uuid,
    task: &crate::db::execution_environments::NativeTask,
) -> Result<(), TaskExecutionError> {
    let job_id = task.prepare_job_id.ok_or(TaskExecutionError::Conflict)?;
    let proof = queries::find_job(tx, application_id, environment_id, job_id)
        .await?
        .ok_or(TaskExecutionError::Conflict)?;
    let Some((_, _, _)) = queries::seed_identity(tx, task.task_id).await? else {
        return Err(TaskExecutionError::Conflict);
    };
    if proof.job_kind != "prepare_native_task"
        || proof.native_task_id != Some(task.task_id)
        || proof.assigned_manager_id != task.assigned_manager_id
        || proof.target_generation != task.target_generation
        || proof.state != "succeeded"
        || proof.result.as_ref().and_then(|value| value.get("outcome"))
            != Some(&serde_json::Value::String("task_prepared".to_string()))
    {
        return Err(TaskExecutionError::Conflict);
    }
    Ok(())
}

async fn validate_environment_candidate(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    application_id: i64,
    environment_id: Uuid,
    environment: &crate::db::execution_environments::tasks::TaskEnvironment,
    manager_id: i64,
) -> Result<(), TaskExecutionError> {
    let Some(candidate) = environment.container_generation else {
        return if environment.state == "absent" {
            Ok(())
        } else {
            Err(TaskExecutionError::Conflict)
        };
    };
    if environment.state != "replaced"
        || environment.generation <= candidate
        || environment.stop_proof_generation != Some(candidate)
    {
        return Err(TaskExecutionError::Conflict);
    }
    let proof_id = environment
        .stop_proof_job_id
        .ok_or(TaskExecutionError::Conflict)?;
    let proof = queries::find_job(tx, application_id, environment_id, proof_id).await?;
    if !crate::db::execution_environments::lifecycle::proof_is_confirmed_stop(
        proof.as_ref(),
        manager_id,
        candidate,
    ) {
        return Err(TaskExecutionError::Conflict);
    }
    Ok(())
}

async fn validate_no_legacy_blockers(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
) -> Result<(), TaskExecutionError> {
    let jobs = queries::load_jobs(tx, application_id, environment_id).await?;
    if jobs.iter().any(|job| {
        matches!(job.state.as_str(), "pending" | "claimed" | "uncertain")
            && ((job.native_task_id.is_none()
                && matches!(job.job_kind.as_str(), "ensure" | "stop"))
                || (job.native_task_id == Some(task_id)
                    && matches!(job.job_kind.as_str(), "start_native_task" | "stop")))
    }) {
        return Err(TaskExecutionError::Conflict);
    }
    Ok(())
}
