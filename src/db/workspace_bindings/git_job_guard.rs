//! Git-job guard for every legacy workspace writer and lifecycle transition.
//!
//! The caller must already hold the binding row lock in its open transaction.
//! The guard is re-read after that lock, in the same transaction, so a Git job
//! admitted while the caller waited is visible. Active includes pending,
//! claimed, and uncertain; no grace period releases uncertain rows.
//!
//! The job id is for internal logging only. Transport callers receive the
//! existing busy/conflict shape, never a job id or state.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

const GUARD_SQL: &str = include_str!("../../sql/workspace_git_jobs/git_job_workspace_guard.sql");
pub(crate) async fn git_job_holds_binding(
    tx: &mut Transaction<'_, Postgres>,
    workspace_binding_id: i64,
) -> anyhow::Result<Option<Uuid>> {
    let active: Option<(Uuid,)> = sqlx::query_as(GUARD_SQL)
        .bind(workspace_binding_id)
        .fetch_optional(&mut **tx)
        .await?;
    if let Some((job_id,)) = active {
        tracing::info!(
            job_id = %job_id,
            workspace_binding_id,
            "workspace operation refused: a git job holds this workspace"
        );
    }
    Ok(active.map(|(job_id,)| job_id))
}

#[cfg(test)]
mod tests {
    use super::GUARD_SQL;

    #[test]
    fn guard_counts_pending_claimed_and_uncertain() {
        let sql = GUARD_SQL.to_ascii_uppercase();
        for state in ["'PENDING'", "'CLAIMED'", "'UNCERTAIN'"] {
            assert!(sql.contains(state), "guard must count {state}");
        }
        assert!(sql.contains("WORKSPACE_BINDING_ID = $1"));
        assert!(sql.contains("LIMIT 1"));
    }
}
