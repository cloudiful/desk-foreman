//! Scheduling one durable native-task preparation (issue 753 P2b).
//!
//! One owner-scoped transaction with the P1 lock order — environment
//! `FOR UPDATE`, then task, then binding — never reversed. Scheduling is a
//! control-plane act only: it records a durable intent for the task's original
//! manager and moves the task to `preparing`. It dispatches nothing, mounts
//! nothing and starts no container.
//!
//! Every fence is checked before any write, the job row and the task's
//! `prepare_job_id` link commit together, and a replay returns the existing
//! durable job with its *current* status rather than enqueueing a second one.

use sqlx::PgPool;
use uuid::Uuid;

use super::types::{PreparationDescriptor, PrepareError, PrepareRequest, internal};
use crate::db::execution_environment_jobs::ExecutionEnvironmentJob;
use crate::db::execution_environments::tasks::queries::{
    lock_binding, lock_environment, lock_task,
};
use crate::db::execution_environments::tasks::seeds::find_native_task_seed_header;
use crate::db::execution_environments::tasks::task_token_is_valid;

/// Manager heartbeat window, matching the existing selection semantics.
const MANAGER_HEARTBEAT_TTL_SECS: i64 = runner_protocol::RUNNER_MANAGER_HEARTBEAT_TTL_SECS as i64;

pub(super) async fn schedule(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
    request: &PrepareRequest,
) -> Result<PreparationDescriptor, PrepareError> {
    if !task_token_is_valid(&request.operation_token) {
        return Err(PrepareError::TokenConflict);
    }

    let mut tx = pool.begin().await.map_err(internal)?;

    let environment = lock_environment(&mut tx, application_id, environment_id)
        .await
        .map_err(internal)?
        .ok_or(PrepareError::NotFound)?;
    if request.expected_generation != environment.generation {
        return Err(PrepareError::StaleGeneration);
    }

    let task = lock_task(&mut tx, application_id, environment_id, task_id)
        .await
        .map_err(internal)?
        .ok_or(PrepareError::NotFound)?;

    // A durable preparation already exists: it is the authority. An identical
    // token replays it with its current status; a different token under an
    // existing preparation is a conflict. Never enqueue a second one.
    if let Some(existing) = find_prepare_job(&mut tx, task_id).await.map_err(internal)? {
        if existing.token != request.operation_token {
            return Err(PrepareError::TokenConflict);
        }
        let descriptor = descriptor_for(&task.state, &existing);
        tx.commit().await.map_err(internal)?;
        return Ok(descriptor);
    }

    // Only the environment's active admitted intent may be prepared.
    if task.state != "admitted" || environment.active_native_task_id != Some(task_id) {
        return Err(PrepareError::TaskNotAdmitted);
    }
    if task.container_generation.is_some() || environment.container_generation.is_some() {
        return Err(PrepareError::EnvironmentNotIdle);
    }
    // The task's subject must still be the environment's own subject: the
    // environment owns the authoritative subject and a task is never prepared
    // for a subject it no longer matches.
    if task.subject_external_user_id != environment.external_user_id {
        return Err(PrepareError::SubjectMismatch);
    }

    let binding = lock_binding(&mut tx, task.workspace_binding_id)
        .await
        .map_err(internal)?
        .ok_or(PrepareError::NotFound)?;
    if binding.application_id != application_id || !binding.is_active_code_project() {
        return Err(PrepareError::BindingIneligible);
    }

    // The sealed input must exist: a preparation with nothing to materialize is
    // not a valid intent.
    if !find_native_task_seed_header(&mut tx, task_id)
        .await
        .map_err(internal)?
    {
        return Err(PrepareError::SeedMissing);
    }

    // No unresolved ensure/stop operation may survive scheduling.
    let jobs = crate::db::execution_environments::lifecycle::load_jobs(
        &mut tx,
        application_id,
        environment_id,
    )
    .await
    .map_err(internal)?;
    if jobs.iter().any(|job| {
        matches!(job.job_kind.as_str(), "ensure" | "stop")
            && matches!(job.state.as_str(), "pending" | "claimed" | "uncertain")
    }) {
        return Err(PrepareError::UnresolvedBlockers);
    }

    // The original manager is never reassigned: bind to the task's own manager
    // and require it still enabled, online and recently seen.
    let manager_id = task.assigned_manager_id;
    if !manager_is_usable(&mut tx, manager_id, MANAGER_HEARTBEAT_TTL_SECS)
        .await
        .map_err(internal)?
    {
        return Err(PrepareError::ManagerUnavailable);
    }

    let job_id = Uuid::new_v4();
    let deadline = chrono::Utc::now() + chrono::Duration::seconds(300);
    let inserted = insert_prepare_job(
        &mut tx,
        job_id,
        &request.operation_token,
        manager_id,
        environment.generation,
        task_id,
        deadline,
        environment_id,
        application_id,
    )
    .await
    .map_err(internal)?;

    let Some(job) = inserted else {
        // A racing schedule won on the token or the active-preparation index.
        let winner = find_prepare_job(&mut tx, task_id)
            .await
            .map_err(internal)?
            .ok_or(PrepareError::TokenConflict)?;
        if winner.token != request.operation_token {
            return Err(PrepareError::TokenConflict);
        }
        let descriptor = descriptor_for(&task.state, &winner);
        tx.commit().await.map_err(internal)?;
        return Ok(descriptor);
    };

    // The task moves to `preparing` and links the job in the same commit, so a
    // task is never `preparing` without its job or vice versa.
    let prepared =
        mark_task_preparing(&mut tx, application_id, environment_id, task_id, job.job_id)
            .await
            .map_err(internal)?
            .ok_or(PrepareError::TaskNotAdmitted)?;

    tx.commit().await.map_err(internal)?;
    Ok(descriptor_for(&prepared.state, &job))
}

/// Build the public descriptor from a task state and the durable job.
fn descriptor_for(task_state: &str, job: &ExecutionEnvironmentJob) -> PreparationDescriptor {
    PreparationDescriptor {
        task_id: job.native_task_id.unwrap_or_default(),
        task_state: task_state.to_string(),
        job_id: job.job_id,
        job_state: job.state.clone(),
        target_generation: job.target_generation,
        active: crate::db::execution_environments::NATIVE_TASK_ACTIVE_STATES.contains(&task_state),
        released: task_state == "released",
    }
}

/// True when the exact manager is still usable for preparation.
async fn manager_is_usable(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    manager_id: i64,
    heartbeat_ttl_secs: i64,
) -> anyhow::Result<bool> {
    let row: Option<i64> = sqlx::query_file_scalar!(
        "src/sql/execution_environments/tasks/prepare/is_manager_usable.sql",
        manager_id,
        heartbeat_ttl_secs as f64,
    )
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.is_some())
}

async fn find_prepare_job(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    task_id: Uuid,
) -> anyhow::Result<Option<ExecutionEnvironmentJob>> {
    Ok(sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environments/tasks/prepare/find_prepare_job_by_task.sql",
        task_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

#[allow(clippy::too_many_arguments)]
async fn insert_prepare_job(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job_id: Uuid,
    token: &str,
    manager_id: i64,
    generation: i64,
    task_id: Uuid,
    deadline: chrono::DateTime<chrono::Utc>,
    environment_id: Uuid,
    application_id: i64,
) -> anyhow::Result<Option<ExecutionEnvironmentJob>> {
    Ok(sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environments/tasks/prepare/insert_prepare_job.sql",
        job_id,
        token,
        manager_id,
        generation,
        task_id,
        deadline,
        environment_id,
        application_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

async fn mark_task_preparing(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
    job_id: Uuid,
) -> anyhow::Result<Option<crate::db::execution_environments::NativeTask>> {
    Ok(sqlx::query_file_as!(
        crate::db::execution_environments::NativeTask,
        "src/sql/execution_environments/tasks/prepare/mark_task_preparing.sql",
        task_id,
        application_id,
        environment_id,
        job_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}
