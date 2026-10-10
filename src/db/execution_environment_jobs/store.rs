//! Persistence for durable environment-operation records (issue 741 P1).
//!
//! This module owns enqueue plus the owner-scoped reads. Claim, completion and
//! expiry live in [`super::lifecycle`]. Every caller-facing operation scopes
//! application and environment together, so a valid but foreign owner pair
//! cannot read or mutate another application's row: ownership is enforced in
//! SQL, never trusted from the caller. Static SQL lives under
//! `src/sql/execution_environment_jobs/` and is checked with `query_file_as!`.
//! No dispatch layer consumes rows here.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::types::{ExecutionEnvironmentJob, job_kind_is_valid, job_token_is_valid};

/// Outcome of an idempotent enqueue.
#[derive(Debug)]
pub enum EnqueueOutcome {
    /// A new job row was inserted.
    Created(ExecutionEnvironmentJob),
    /// The same token replayed an identical intent; the original row is
    /// returned unchanged.
    Replayed(ExecutionEnvironmentJob),
    /// The caller's own token exists with a different intent; nothing was
    /// mutated.
    TokenConflict { current: ExecutionEnvironmentJob },
    /// This environment already holds an active operation of the same kind.
    /// No existing row is exposed, so another owner's record never leaks.
    ActiveConflict,
    /// The application does not own the environment, the assigned manager is
    /// missing/disabled, or the token is held by another tenant. No row is
    /// exposed in any of these cases.
    EnvironmentNotFound,
}

/// True when a database error is a unique-constraint violation (`23505`).
pub(super) fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db) if db.code().as_deref() == Some("23505"))
}

/// Enqueue one environment operation intent.
///
/// `assigned_manager_id` is validated as an existing enabled manager inline
/// and fixed forever. Tenancy: the insert selects from `execution_environments`
/// filtered by the owning application, so a foreign pair inserts nothing and
/// never observes another owner's row. A same-token identical replay returns
/// the original row; a same-owner token with a different intent rejects.
pub async fn enqueue_execution_environment_job(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    assigned_manager_id: i64,
    job_kind: &str,
    target_generation: i64,
    token: &str,
    deadline_at: DateTime<Utc>,
) -> anyhow::Result<EnqueueOutcome> {
    if !job_kind_is_valid(job_kind) {
        anyhow::bail!("invalid environment job kind: {job_kind}");
    }
    if target_generation < 0 {
        anyhow::bail!("environment job target generation must be >= 0");
    }
    if !job_token_is_valid(token) {
        anyhow::bail!("environment job token must be non-blank and bounded");
    }
    let job_id = Uuid::new_v4();

    let inserted: Option<ExecutionEnvironmentJob> = match sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environment_jobs/insert_execution_environment_job.sql",
        job_id,
        token,
        assigned_manager_id,
        job_kind,
        target_generation,
        deadline_at,
        environment_id,
        application_id,
    )
    .fetch_optional(pool)
    .await
    {
        Ok(row) => row,
        Err(error) if is_unique_violation(&error) => {
            // A same-kind active operation already exists. Never return that
            // row: it may belong to another caller.
            if find_active_execution_environment_job(pool, application_id, environment_id)
                .await?
                .is_some()
            {
                return Ok(EnqueueOutcome::ActiveConflict);
            }
            return Err(error.into());
        }
        Err(error) => return Err(error.into()),
    };

    if let Some(row) = inserted {
        return Ok(EnqueueOutcome::Created(row));
    }

    // No source row (foreign owner pair / missing or disabled manager) and a
    // globally taken token both fall through here. Only a row reachable by the
    // caller's own owner pair is ever returned.
    let Some(current) =
        find_execution_environment_job_by_token(pool, application_id, environment_id, token)
            .await?
    else {
        return Ok(EnqueueOutcome::EnvironmentNotFound);
    };

    let identical = current.assigned_manager_id == assigned_manager_id
        && current.job_kind == job_kind
        && current.target_generation == target_generation;
    if identical {
        Ok(EnqueueOutcome::Replayed(current))
    } else {
        Ok(EnqueueOutcome::TokenConflict { current })
    }
}

/// Owner-scoped lookup by opaque operation token.
pub async fn find_execution_environment_job_by_token(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    token: &str,
) -> anyhow::Result<Option<ExecutionEnvironmentJob>> {
    sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environment_jobs/find_execution_environment_job_by_token.sql",
        token,
        application_id,
        environment_id,
    )
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

/// The environment's active operation blocker, if any: pending, claimed or
/// uncertain of either kind. A present blocker means no new ensure of any
/// generation may be recorded.
pub async fn find_active_execution_environment_job(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
) -> anyhow::Result<Option<ExecutionEnvironmentJob>> {
    sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environment_jobs/find_active_execution_environment_job.sql",
        application_id,
        environment_id,
    )
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}
