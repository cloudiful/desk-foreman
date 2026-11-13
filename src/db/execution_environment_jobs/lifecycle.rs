//! Atomic lifecycle transitions for durable environment-operation records
//! (issue 741 P1).
//!
//! Claim and complete are single-statement compare-and-set updates; expiry
//! fails closed. Every statement scopes application and environment together so
//! a foreign owner pair cannot mutate anything. Static SQL lives under
//! `src/sql/execution_environment_jobs/` and is checked with `query_file_as!`.

use sqlx::PgPool;
use uuid::Uuid;

use super::store::find_execution_environment_job_by_token;
use super::types::{
    ExecutionEnvironmentJob, ExecutionEnvironmentJobResult, job_state_is_terminal,
    redact_error_summary,
};
use crate::db::task_runtime::{JobDisposition, TaskRuntime};

/// Outcome of an atomic claim.
#[derive(Debug)]
pub enum ClaimOutcome {
    /// The caller became the single claimant.
    Claimed(ExecutionEnvironmentJob),
    /// The job is not claimable by this manager (wrong manager, already
    /// claimed, expired, terminal, unknown, or foreign). `current` is the
    /// caller's own row when one exists, never another owner's.
    NotClaimable {
        current: Option<ExecutionEnvironmentJob>,
    },
}

/// Outcome of an atomic complete.
#[derive(Debug)]
pub enum CompleteOutcome {
    /// The claimant recorded a terminal result.
    Completed(ExecutionEnvironmentJob),
    /// A matching already-terminal job returned its existing outcome.
    AlreadyTerminal(ExecutionEnvironmentJob),
    /// The caller's replay contradicts the recorded terminal outcome.
    TerminalConflict { current: ExecutionEnvironmentJob },
    /// No current claim matched this caller; nothing was mutated.
    NotCompletable {
        current: Option<ExecutionEnvironmentJob>,
    },
}

/// Atomically claim a pending job for its assigned manager.
///
/// Exactly one concurrent caller wins; the loser sees `NotClaimable` with the
/// post-winner row. Expired or terminal jobs never claim, and no reclaim of
/// claimed/uncertain jobs exists. The claim and the durable task of the job
/// commit together: a claim without a tracked task never becomes durable.
pub async fn claim_execution_environment_job(
    pool: &PgPool,
    runtime: &TaskRuntime,
    application_id: i64,
    environment_id: Uuid,
    token: &str,
    manager_id: i64,
) -> anyhow::Result<ClaimOutcome> {
    let mut tx = pool.begin().await?;
    let claimed: Option<ExecutionEnvironmentJob> = sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environment_jobs/claim_execution_environment_job.sql",
        token,
        application_id,
        environment_id,
        manager_id,
    )
    .fetch_optional(&mut *tx)
    .await?;

    if let Some(row) = claimed {
        if let Err(error) = runtime
            .track_claim(&mut tx, row.job_id, &row.job_kind, manager_id)
            .await
        {
            tx.rollback().await?;
            return Err(error);
        }
        tx.commit().await?;
        return Ok(ClaimOutcome::Claimed(row));
    }
    tx.rollback().await?;

    let current =
        find_execution_environment_job_by_token(pool, application_id, environment_id, token)
            .await?;
    Ok(ClaimOutcome::NotClaimable { current })
}

/// Atomically complete a claimed job as its assigned claimant.
///
/// The update matches job id, owner pair, token, assigned manager, `claimed`
/// state and an unexpired deadline; anything else mutates nothing. A terminal
/// replay returns the existing outcome only when state, result and error
/// summary are identical, otherwise `TerminalConflict`. Completion after the
/// deadline never succeeds.
pub async fn complete_execution_environment_job(
    pool: &PgPool,
    runtime: &TaskRuntime,
    application_id: i64,
    environment_id: Uuid,
    token: &str,
    job_id: Uuid,
    manager_id: i64,
    state: &str,
    result: Option<ExecutionEnvironmentJobResult>,
    error_summary: Option<&str>,
) -> anyhow::Result<CompleteOutcome> {
    if !job_state_is_terminal(state) {
        anyhow::bail!("complete requires a terminal state, got {state}");
    }
    let result_value = result.as_ref().map(serde_json::to_value).transpose()?;
    let error_summary = error_summary.map(redact_error_summary);

    let mut tx = pool.begin().await?;
    let completed: Option<ExecutionEnvironmentJob> = sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environment_jobs/complete_execution_environment_job.sql",
        job_id,
        application_id,
        environment_id,
        token,
        manager_id,
        state,
        result_value,
        error_summary.as_deref(),
    )
    .fetch_optional(&mut *tx)
    .await?;

    if let Some(row) = completed {
        if let Err(error) = runtime
            .settle(
                &mut tx,
                row.job_id,
                Some(&row.job_kind),
                JobDisposition::for_state(&row.state),
            )
            .await
        {
            tx.rollback().await?;
            return Err(error);
        }
        tx.commit().await?;
        return Ok(CompleteOutcome::Completed(row));
    }
    tx.rollback().await?;

    let Some(current) =
        find_execution_environment_job_by_token(pool, application_id, environment_id, token)
            .await?
    else {
        return Ok(CompleteOutcome::NotCompletable { current: None });
    };

    if job_state_is_terminal(&current.state) {
        let identical = current.state == state
            && current.result == result_value
            && current.error_summary == error_summary;
        if identical {
            Ok(CompleteOutcome::AlreadyTerminal(current))
        } else {
            Ok(CompleteOutcome::TerminalConflict { current })
        }
    } else {
        Ok(CompleteOutcome::NotCompletable {
            current: Some(current),
        })
    }
}

/// The environment of one overdue export job of the authenticated manager.
///
/// The bounded poll preflight uses it to expire at most one overdue export
/// environment before the next claim. A manager that never polls keeps the
/// existing limitation that no global expiry runs.
pub(crate) async fn find_overdue_export_environment_for_manager(
    pool: &PgPool,
    manager_id: i64,
) -> anyhow::Result<Option<OverdueExportEnvironment>> {
    Ok(sqlx::query_file_as!(
        OverdueExportEnvironment,
        "src/sql/execution_environment_jobs/find_overdue_export_environment_for_manager.sql",
        manager_id,
    )
    .fetch_optional(pool)
    .await?)
}

/// The owner pair of one overdue export job of a manager.
#[derive(sqlx::FromRow)]
pub(crate) struct OverdueExportEnvironment {
    pub(crate) application_id: i64,
    pub(crate) environment_id: Uuid,
}

/// Fail closed: every expired pending or claimed job of one owned environment
/// becomes `uncertain` in a single atomic statement. Uncertain jobs are never
/// claimable again and no API clears them.
pub async fn expire_execution_environment_jobs(
    pool: &PgPool,
    runtime: &TaskRuntime,
    application_id: i64,
    environment_id: Uuid,
) -> anyhow::Result<Vec<ExecutionEnvironmentJob>> {
    let mut tx = pool.begin().await?;
    let environment = sqlx::query_file_scalar!(
        "src/sql/execution_environments/tasks/prepare/lock_environment_for_expiry.sql",
        application_id,
        environment_id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    if environment.is_none() {
        tx.commit().await?;
        return Ok(Vec::new());
    }
    crate::db::execution_environments::tasks::native_execution::expire_overdue_native_jobs(
        &mut tx,
        runtime,
        application_id,
        environment_id,
    )
    .await?;

    let expired: Vec<ExecutionEnvironmentJob> = sqlx::query_file_as!(
        ExecutionEnvironmentJob,
        "src/sql/execution_environment_jobs/expire_execution_environment_jobs.sql",
        application_id,
        environment_id,
    )
    .fetch_all(&mut *tx)
    .await?;

    for job in &expired {
        if let Err(error) = runtime
            .settle(
                &mut tx,
                job.job_id,
                Some(&job.job_kind),
                JobDisposition::Failed,
            )
            .await
        {
            tx.rollback().await?;
            return Err(error);
        }
    }

    tx.commit().await?;
    Ok(expired)
}
