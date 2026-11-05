//! Claim-completion, expiry, and replay rules for durable Git jobs.

use sqlx::PgPool;
use uuid::Uuid;

use super::store::find_git_job_by_id;
use super::types::{GitJobCompletion, GitJobRecord, git_job_state_is_terminal, valid_token};

#[derive(Debug)]
pub enum CompleteOutcome {
    Completed(GitJobRecord),
    AlreadyTerminal(GitJobRecord),
    TerminalConflict { current: GitJobRecord },
    NotCompletable { current: Option<GitJobRecord> },
}

pub async fn complete_git_job(
    pool: &PgPool,
    job_id: Uuid,
    token: &str,
    assigned_manager_id: i64,
    completion: GitJobCompletion,
) -> anyhow::Result<CompleteOutcome> {
    if !valid_token(token) || assigned_manager_id <= 0 {
        anyhow::bail!("invalid git job completion input");
    }
    let state = completion.state();
    let result = completion.result_value()?;
    let error_summary = completion.error_summary();
    let cleanup_confirmed = completion.cleanup_confirmed();

    let completed: Option<GitJobRecord> = sqlx::query_file_as!(
        GitJobRecord,
        "src/sql/workspace_git_jobs/complete_git_job.sql",
        job_id,
        token,
        assigned_manager_id,
        state,
        result.as_ref(),
        error_summary.as_deref(),
        cleanup_confirmed,
    )
    .fetch_optional(pool)
    .await?;
    if let Some(row) = completed {
        return Ok(CompleteOutcome::Completed(row));
    }

    let Some(current) = find_git_job_by_id(pool, job_id).await? else {
        return Ok(CompleteOutcome::NotCompletable { current: None });
    };
    if current.token != token || current.assigned_manager_id != assigned_manager_id {
        return Ok(CompleteOutcome::NotCompletable { current: None });
    }
    if git_job_state_is_terminal(&current.state) {
        let identical = current.state == state
            && current.result == result
            && current.error_summary == error_summary
            && current.cleanup_confirmed == cleanup_confirmed;
        return Ok(if identical {
            CompleteOutcome::AlreadyTerminal(current)
        } else {
            CompleteOutcome::TerminalConflict { current }
        });
    }
    Ok(CompleteOutcome::NotCompletable {
        current: Some(current),
    })
}

pub async fn expire_git_jobs(pool: &PgPool) -> anyhow::Result<Vec<GitJobRecord>> {
    sqlx::query_file_as!(
        GitJobRecord,
        "src/sql/workspace_git_jobs/expire_git_jobs.sql"
    )
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

/// Atomically claim the oldest pending job assigned to one manager.
///
/// `FOR UPDATE SKIP LOCKED` yields a single winner under concurrent polls.
/// Expired, claimed, uncertain, terminal, and foreign rows are never claimed.
pub async fn claim_next_git_job(
    pool: &PgPool,
    assigned_manager_id: i64,
) -> anyhow::Result<Option<GitJobRecord>> {
    if assigned_manager_id <= 0 {
        anyhow::bail!("invalid git job manager");
    }
    let claimed: Option<GitJobRecord> = sqlx::query_file_as!(
        GitJobRecord,
        "src/sql/workspace_git_jobs/claim_next_for_assigned_manager.sql",
        assigned_manager_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(claimed)
}

/// Read the exact live credential scope for one assigned manager.
///
/// The row must be claimed by that manager and unexpired; the handler compares
/// the operation id header separately.
pub async fn git_job_credential_scope(
    pool: &PgPool,
    job_id: Uuid,
    assigned_manager_id: i64,
) -> anyhow::Result<Option<GitJobRecord>> {
    if job_id.is_nil() || assigned_manager_id <= 0 {
        anyhow::bail!("invalid git job credential scope");
    }
    let row: Option<GitJobRecord> = sqlx::query_file_as!(
        GitJobRecord,
        "src/sql/workspace_git_jobs/git_job_credential_scope.sql",
        job_id,
        assigned_manager_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}
