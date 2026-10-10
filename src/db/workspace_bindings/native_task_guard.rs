//! Native-task guard for every legacy workspace writer (issue 753 P1).
//!
//! A durable native task and the legacy workspace writers may never overlap:
//! both would write the same workspace with no shared fence. This is the one
//! place that knows how to detect that overlap, so every legacy path calls it
//! instead of re-deriving the rule.
//!
//! The contract callers must honour:
//!
//! - the binding row lock must already be held in the caller's open
//!   transaction;
//! - the check must be read **after** that lock was granted, inside that same
//!   transaction. A `NOT EXISTS` evaluated in the snapshot taken before the
//!   lock is exactly the interleaving this guard exists to close: a task
//!   admitted while we waited on the lock would be invisible;
//! - a conflict is reported as "no workspace", never as an error that names
//!   the task, its binding or its state.
//!
//! A returned `Some(task_id)` means a native task owns this workspace. The
//! caller refuses; it never clears, releases or otherwise touches the task,
//! its lease, or the binding's directory.

use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

/// Shared guard SQL. Re-read under the caller's held binding lock.
const GUARD_SQL: &str = include_str!("../../sql/native_task_workspace_guard.sql");

/// True while a native task holds this workspace binding.
///
/// Must run inside the caller's transaction, after its binding row lock. The
/// returned id is for logging only: it is never returned to a transport
/// caller, because it would confirm the existence of another tenant's task.
pub(crate) async fn native_task_holds_binding(
    tx: &mut Transaction<'_, Postgres>,
    workspace_binding_id: i64,
) -> anyhow::Result<Option<Uuid>> {
    let active: Option<(Uuid,)> = sqlx::query_as(GUARD_SQL)
        .bind(workspace_binding_id)
        .fetch_optional(&mut **tx)
        .await?;
    if let Some((task_id,)) = active {
        tracing::info!(
            task_id = %task_id,
            workspace_binding_id,
            "legacy workspace write refused: a native task holds this workspace"
        );
    }
    Ok(active.map(|(task_id,)| task_id))
}

#[cfg(test)]
mod tests {
    use super::GUARD_SQL;

    #[test]
    fn the_guard_matches_the_partial_unique_index_state_set() {
        let sql = GUARD_SQL.to_ascii_uppercase();
        for state in [
            "'ADMITTED'",
            "'PREPARING'",
            "'PREPARED'",
            "'RUNNING'",
            "'STOPPING'",
            "'UNCERTAIN'",
        ] {
            assert!(
                sql.contains(state),
                "guard must count {state} as an active task"
            );
        }
        assert!(
            !sql.contains("'RELEASED'"),
            "a released task must not block a legacy writer"
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
            "a NOT EXISTS evaluated in the snapshot would miss a task admitted \
             while we waited on the binding row lock"
        );
        assert!(
            body.contains("LIMIT 1"),
            "the guard resolves to the single active task that owns the workspace"
        );
    }
}

/// Apply the existing lifecycle state transition on a guarded transaction.
///
/// The statement itself is the unchanged `set_workspace_binding_state.sql`;
/// only its execution moves onto the caller's transaction so the state change
/// happens while the binding row lock this guard took is still held. Running it
/// on a second connection would leave exactly the window the guard closes.
pub(crate) async fn set_lifecycle_state(
    tx: &mut Transaction<'_, Postgres>,
    workspace_binding_id: i64,
    lifecycle_state: &str,
) -> anyhow::Result<Option<crate::db::types::WorkspaceBindingResponse>> {
    Ok(
        sqlx::query_as::<_, crate::db::types::WorkspaceBindingResponse>(include_str!(
            "../../sql/set_workspace_binding_state.sql"
        ))
        .bind(workspace_binding_id)
        .bind(lifecycle_state)
        .fetch_optional(&mut **tx)
        .await?,
    )
}

/// Delete an archived binding on a guarded transaction.
///
/// The unchanged `delete_workspace_binding.sql`, executed on the caller's
/// transaction so the row cannot be removed after the guard released the lock.
pub(crate) async fn delete_binding(
    tx: &mut Transaction<'_, Postgres>,
    workspace_binding_id: i64,
) -> anyhow::Result<Option<String>> {
    Ok(
        sqlx::query(include_str!("../../sql/delete_workspace_binding.sql"))
            .bind(workspace_binding_id)
            .fetch_optional(&mut **tx)
            .await?
            .map(|row| row.get("workspace_root")),
    )
}
