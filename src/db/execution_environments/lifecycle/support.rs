//! Focused transaction helpers for the lifecycle orchestration (issue 742 P3).
//!
//! Every helper runs on an already-open owner-scoped transaction; the caller
//! holds the environment row lock, so read/modify sequences cannot interleave
//! with another lifecycle mutation of the same environment. Static SQL lives
//! under `src/sql/execution_environments/lifecycle/`.

use chrono::Utc;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::db::execution_environment_jobs::ExecutionEnvironmentJob;

use super::types::LifecycleEnvironment;

/// Heartbeat window for internal manager selection, matching the existing
/// selection query semantics.
const MANAGER_HEARTBEAT_TTL_SECS: f64 = runner_protocol::RUNNER_MANAGER_HEARTBEAT_TTL_SECS as f64;

/// Durable operation deadline for one lifecycle job. An overdue pending or
/// claimed job fails closed to `uncertain`; it never becomes a running or stop
/// confirmation.
const LIFECYCLE_JOB_DEADLINE_SECS: i64 = 300;

/// One new durable operation intent.
pub(crate) struct NewLifecycleJob<'a> {
    pub(crate) token: &'a str,
    pub(crate) environment_id: Uuid,
    pub(crate) application_id: i64,
    pub(crate) assigned_manager_id: i64,
    pub(crate) job_kind: &'a str,
    pub(crate) target_generation: i64,
}

/// Lock and read the owner-scoped environment row.
pub(crate) async fn lock_environment(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
) -> anyhow::Result<Option<LifecycleEnvironment>> {
    Ok(sqlx::query_file_as!(
        LifecycleEnvironment,
        "src/sql/execution_environments/lifecycle/lock_lifecycle_environment.sql",
        environment_id,
        application_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// Fail closed: overdue pending/claimed rows of the environment become
/// `uncertain` before any state decision.
pub(crate) async fn expire_overdue_jobs(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
) -> anyhow::Result<Vec<ExecutionEnvironmentJob>> {
    // Lifecycle callers invoke expiry before their later environment read. Take
    // the owner lock here first so no job row is held while a task completion
    // waits to acquire this environment.
    let environment = sqlx::query_file_scalar!(
        "src/sql/execution_environments/tasks/prepare/lock_environment_for_expiry.sql",
        application_id,
        environment_id,
    )
    .fetch_optional(&mut **tx)
    .await?;
    if environment.is_none() {
        return Ok(Vec::new());
    }

    let expired = sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environments/lifecycle/expire_lifecycle_jobs.sql",
        application_id,
        environment_id,
    )
    .fetch_all(&mut **tx)
    .await?;

    // A preparation that expired retains its task as `uncertain` in the same
    // transaction: the task keeps its exclusive environment and workspace
    // anchors, is never released and is never re-dispatched. Expiry alone can
    // never make an environment ready.
    for job in &expired {
        if job.job_kind == "prepare_native_task" {
            if let Some(task_id) = job.native_task_id {
                crate::db::execution_environments::tasks::prepare::mark_preparation_uncertain(
                    tx,
                    application_id,
                    environment_id,
                    task_id,
                )
                .await?;
            }
        }
    }

    Ok(expired)
}

/// All durable operation rows of one owned environment.
pub(crate) async fn load_jobs(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
) -> anyhow::Result<Vec<ExecutionEnvironmentJob>> {
    Ok(sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environments/lifecycle/find_lifecycle_jobs.sql",
        application_id,
        environment_id,
    )
    .fetch_all(&mut **tx)
    .await?)
}

/// One durable operation row by stable UUID.
pub(crate) async fn find_job_by_id(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    job_id: Uuid,
) -> anyhow::Result<Option<ExecutionEnvironmentJob>> {
    Ok(sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environments/lifecycle/find_lifecycle_job_by_id.sql",
        application_id,
        environment_id,
        job_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// One durable operation row by opaque replay token.
pub(crate) async fn find_job_by_token(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    token: &str,
) -> anyhow::Result<Option<ExecutionEnvironmentJob>> {
    Ok(sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environments/lifecycle/find_lifecycle_job_by_token.sql",
        application_id,
        environment_id,
        token,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// Insert one durable operation intent. `None` means the token already exists
/// (a same-token replay the caller must reconcile).
pub(crate) async fn insert_job(
    tx: &mut Transaction<'_, Postgres>,
    job: NewLifecycleJob<'_>,
) -> anyhow::Result<Option<ExecutionEnvironmentJob>> {
    let job_id = Uuid::new_v4();
    let deadline = Utc::now() + chrono::Duration::seconds(LIFECYCLE_JOB_DEADLINE_SECS);
    Ok(sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environments/lifecycle/insert_lifecycle_job.sql",
        job_id,
        job.token,
        job.environment_id,
        job.application_id,
        job.assigned_manager_id,
        job.job_kind,
        job.target_generation,
        deadline,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// Internally select the enabled+online+recently-seen manager. Used only for a
/// first provision with no binding yet.
pub(crate) async fn select_manager(
    tx: &mut Transaction<'_, Postgres>,
) -> anyhow::Result<Option<i64>> {
    Ok(sqlx::query_file_scalar!(
        "src/sql/execution_environments/lifecycle/select_lifecycle_manager.sql",
        MANAGER_HEARTBEAT_TTL_SECS,
    )
    .fetch_optional(&mut **tx)
    .await?)
}
