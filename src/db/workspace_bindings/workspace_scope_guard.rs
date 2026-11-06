//! Workspace-scope guard for every workspace writer and lifecycle transition
//! (issue 1180 P2).
//!
//! A durable workspace-scoped execution environment and a legacy workspace
//! writer may never overlap: both would write the same workspace with no shared
//! fence. This is the one place that knows how to detect that overlap, so every
//! legacy path calls it instead of re-deriving the rule.
//!
//! The contract callers must honour:
//!
//! - the binding row lock must already be held in the caller's open
//!   transaction;
//! - the check must be read **after** that lock was granted, inside that same
//!   transaction. A snapshot taken before the lock would miss an environment
//!   anchored while we waited;
//! - a conflict is reported as the existing busy/conflict shape, never as an
//!   error that names the environment, its binding or its state.
//!
//! A returned `Some(environment_id)` means a scoped environment owns this
//! workspace. The caller refuses; it never clears, releases or otherwise
//! touches the anchor.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

/// Shared guard SQL. Re-read under the caller's held binding lock.
const GUARD_SQL: &str = include_str!("../../sql/workspace_scope_workspace_guard.sql");

/// True while a workspace-scoped environment still owns this binding.
///
/// Must run inside the caller's transaction, after its binding row lock. The
/// returned id is for logging only: it is never returned to a transport
/// caller, because it would confirm the existence of another tenant's
/// environment.
pub(crate) async fn workspace_scope_holds_binding(
    tx: &mut Transaction<'_, Postgres>,
    workspace_binding_id: i64,
) -> anyhow::Result<Option<Uuid>> {
    let active: Option<(Uuid,)> = sqlx::query_as(GUARD_SQL)
        .bind(workspace_binding_id)
        .fetch_optional(&mut **tx)
        .await?;
    if let Some((environment_id,)) = active {
        tracing::info!(
            environment_id = %environment_id,
            workspace_binding_id,
            "workspace operation refused: a scoped environment is busy for this workspace"
        );
    }
    Ok(active.map(|(environment_id,)| environment_id))
}

#[cfg(test)]
mod tests {
    use super::GUARD_SQL;

    #[test]
    fn the_guard_matches_the_busy_state_set() {
        let sql = GUARD_SQL.to_ascii_uppercase();
        for state in [
            "'STARTING'",
            "'READY'",
            "'STOPPING'",
            "'UNREACHABLE'",
            "'UNCERTAIN'",
        ] {
            assert!(
                sql.contains(state),
                "guard must count {state} as a busy scoped environment"
            );
        }
        assert!(
            !sql.contains("'ABSENT'"),
            "an absent environment must not block a workspace writer"
        );
        assert!(
            !sql.contains("'REPLACED'"),
            "a replaced environment must not block a workspace writer"
        );
    }

    /// The executable statement, without the comments that explain it.
    fn guard_body() -> String {
        GUARD_SQL
            .lines()
            .filter(|line| !line.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_uppercase()
    }

    #[test]
    fn the_guard_is_scoped_to_one_binding() {
        let body = guard_body();
        assert!(body.contains("WORKSPACE_BINDING_ID = $1"));
        assert!(
            !body.contains("NOT EXISTS"),
            "a NOT EXISTS evaluated in the snapshot would miss an environment \
             anchored while we waited on the binding row lock"
        );
        assert!(
            body.contains("LIMIT 1"),
            "the guard resolves to the single busy environment for the workspace"
        );
    }
}
