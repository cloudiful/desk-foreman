use sqlx::PgPool;
use uuid::Uuid;

use super::native_execution::types::NativeTaskStopDisposition;
use super::native_execution::{
    NativeTaskOperationDescriptor, TaskExecutionError, TaskExecutionRequest,
};
use super::native_execution::{queries, types};
use crate::db::execution_environment_jobs::{
    ExecutionEnvironmentJob, ExecutionEnvironmentJobResult, job_token_is_valid,
};

const JOB_DEADLINE_SECS: i64 = 300;
const SUPERSEDED_RESULT: &str = "superseded_for_native_task_stop";

pub(super) async fn stop(
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
    let (environment, task, _binding) =
        queries::lock_context(&mut tx, application_id, environment_id, task_id).await?;

    if let Some(stop_job_id) = task.stop_job_id {
        let existing = queries::lock_job(&mut tx, application_id, environment_id, stop_job_id)
            .await?
            .ok_or(TaskExecutionError::Conflict)?;
        if existing.token == request.operation_token {
            let candidate = task.container_generation.unwrap_or(task.target_generation);
            if !is_stop_for_task(&existing, task_id)
                || existing.assigned_manager_id != task.assigned_manager_id
                || existing.target_generation != candidate
                || environment.assigned_manager_id != Some(existing.assigned_manager_id)
                || existing.admission_generation != Some(request.expected_generation)
            {
                return Err(TaskExecutionError::TokenConflict);
            }
            let mut descriptor = NativeTaskOperationDescriptor::new(&environment, &task, &existing);
            descriptor.stop_disposition = Some(NativeTaskStopDisposition::Replay);
            tx.commit().await.map_err(types::internal)?;
            return Ok(descriptor);
        }
        if !matches!(existing.state.as_str(), "uncertain" | "failed")
            || task.state != "uncertain"
            || environment.state != "uncertain"
        {
            return Err(TaskExecutionError::Conflict);
        }
    }

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
    if request.expected_generation != environment.generation {
        return Err(TaskExecutionError::StaleGeneration);
    }
    if environment.active_native_task_id != Some(task_id)
        || environment.assigned_manager_id != Some(task.assigned_manager_id)
        || task.is_released()
        || !matches!(
            task.state.as_str(),
            "prepared" | "preparing" | "starting" | "running" | "stopping" | "uncertain"
        )
    {
        return Err(TaskExecutionError::Conflict);
    }
    if task.subject_external_user_id != environment.external_user_id {
        return Err(TaskExecutionError::Conflict);
    }

    let candidate = task.container_generation.unwrap_or(task.target_generation);
    if task.target_generation != candidate
        || environment
            .container_generation
            .is_some_and(|stored| task.container_generation.is_some() && stored != candidate)
    {
        return Err(TaskExecutionError::Conflict);
    }
    if task.container_generation.is_none() {
        validate_previous_candidate(
            &mut tx,
            application_id,
            environment_id,
            &environment,
            task.assigned_manager_id,
            candidate,
        )
        .await?;
    } else if environment.container_generation != Some(candidate) {
        return Err(TaskExecutionError::Conflict);
    }

    let initial_stop = environment.generation == candidate;
    if initial_stop {
        if !matches!(
            environment.state.as_str(),
            "absent" | "replaced" | "starting" | "ready" | "stopping" | "uncertain"
        ) {
            return Err(TaskExecutionError::Conflict);
        }
    } else if environment.generation < candidate
        || !matches!(environment.state.as_str(), "stopping" | "uncertain")
        || task.container_generation != Some(candidate)
    {
        return Err(TaskExecutionError::Conflict);
    }

    validate_no_legacy_blockers(&mut tx, application_id, environment_id, task_id).await?;
    if !queries::manager_is_usable(&mut tx, task.assigned_manager_id).await? {
        return Err(TaskExecutionError::ManagerUnavailable);
    }

    // Fence late preparation/start acknowledgments before recording the stop.
    let uncertain = serde_json::to_value(ExecutionEnvironmentJobResult {
        outcome: SUPERSEDED_RESULT.to_string(),
        observed_state: None,
        confirmed: None,
        preparation_quiescent: None,
    })
    .map_err(types::internal)?;
    let error_summary = "native task stop superseded an outstanding preparation or start";
    let _: Vec<Uuid> = sqlx::query_file_scalar!(
        "src/sql/execution_environments/tasks/stop/mark_inflight_task_jobs_uncertain.sql",
        task_id,
        uncertain,
        error_summary,
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(types::internal)?;

    let job_id = Uuid::new_v4();
    let job = queries::insert_job(
        &mut tx,
        job_id,
        &request.operation_token,
        environment_id,
        application_id,
        task.assigned_manager_id,
        "stop",
        candidate,
        task_id,
        Some(request.expected_generation),
        types::deadline_after(JOB_DEADLINE_SECS),
    )
    .await?;
    let Some(job) = job else {
        return Err(TaskExecutionError::TokenConflict);
    };

    let old_candidate = environment.container_generation;
    let environment_updated = if initial_stop {
        sqlx::query_file_scalar!(
            "src/sql/execution_environments/tasks/stop/begin_environment_stop.sql",
            environment_id,
            application_id,
            task_id,
            environment.generation,
            candidate,
            old_candidate,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(types::internal)?
    } else {
        sqlx::query_file_scalar!(
            "src/sql/execution_environments/tasks/stop/resume_environment_stop.sql",
            environment_id,
            application_id,
            task_id,
            environment.generation,
            candidate,
            old_candidate,
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(types::internal)?
    };
    if environment_updated.is_none() {
        return Err(TaskExecutionError::Conflict);
    }

    let stopping = sqlx::query_file_as!(
        crate::db::execution_environments::NativeTask,
        "src/sql/execution_environments/tasks/stop/mark_task_stopping.sql",
        task_id,
        application_id,
        environment_id,
        candidate,
        job.job_id,
        task.stop_job_id,
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(types::internal)?
    .ok_or(TaskExecutionError::Conflict)?;

    let updated_environment =
        super::queries::lock_environment(&mut tx, application_id, environment_id)
            .await
            .map_err(types::internal)?
            .ok_or(TaskExecutionError::NotFound)?;
    let mut descriptor = NativeTaskOperationDescriptor::new(&updated_environment, &stopping, &job);
    descriptor.stop_disposition = Some(NativeTaskStopDisposition::Admitted {
        cancel_through_generation: candidate,
    });
    tx.commit().await.map_err(types::internal)?;
    Ok(descriptor)
}

fn is_stop_for_task(job: &ExecutionEnvironmentJob, task_id: Uuid) -> bool {
    job.job_kind == "stop" && job.native_task_id == Some(task_id)
}

async fn validate_previous_candidate(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    application_id: i64,
    environment_id: Uuid,
    environment: &crate::db::execution_environments::tasks::TaskEnvironment,
    manager_id: i64,
    new_candidate: i64,
) -> Result<(), TaskExecutionError> {
    let Some(old_candidate) = environment.container_generation else {
        return if environment.state == "absent" || environment.state == "replaced" {
            Ok(())
        } else {
            Err(TaskExecutionError::Conflict)
        };
    };
    if old_candidate >= new_candidate
        || environment.state != "replaced"
        || environment.stop_proof_generation != Some(old_candidate)
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
        old_candidate,
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
        (job.native_task_id.is_none()
            && matches!(job.job_kind.as_str(), "ensure" | "stop")
            && matches!(job.state.as_str(), "pending" | "claimed" | "uncertain"))
            || (job.native_task_id == Some(task_id)
                && job.job_kind == "stop"
                && matches!(job.state.as_str(), "pending" | "claimed"))
    }) {
        return Err(TaskExecutionError::Conflict);
    }
    Ok(())
}
