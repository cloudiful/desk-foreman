//! Bounded out-of-poll workspace activity observation under the binding fence
//! (issue 1180).
//!
//! Before a scoped provision/replace intent starts its anchor, every
//! configured manager is asked for its in-flight workspace activity through
//! the existing targeted runner seam while the caller still holds the binding
//! row lock. A single absolute timeout covers the manager snapshot and all
//! observations. This is a bounded contention window, never an unbounded
//! network-held lock, and it runs from the application request rather than a
//! manager poll handler so no poll loop waits on itself.
//!
//! Activity includes preparing/registered-but-not-yet-polled work and live
//! Shell sessions. Failures are fail-closed: an empty manager snapshot, a
//! manager that cannot be reached, a mismatched reply, a listing error or the
//! timeout all refuse the intent. Nothing is cancelled, cleaned up or
//! persisted as quiescence evidence.

use std::time::Duration;

use sqlx::{Postgres, Transaction};

use crate::runner::RunnerService;

use super::LifecycleError;

/// Absolute budget for the manager snapshot and every session observation.
const SCOPE_SESSION_PROBE_BUDGET: Duration = Duration::from_secs(10);

/// Observe every configured manager while the caller holds the binding fence.
///
/// The caller must keep its transaction and binding row lock open through this
/// call. Returns `ScopeBusy` when any manager reports a session owned by the
/// binding, and `ScopeManagerUnavailable` on any incomplete observation.
pub(crate) async fn ensure_no_scope_sessions(
    tx: &mut Transaction<'_, Postgres>,
    runner: &dyn RunnerService,
    workspace_binding_id: i64,
) -> Result<(), LifecycleError> {
    let probe = async {
        let manager_ids = sqlx::query_file_scalar!("src/sql/list_scope_session_managers.sql")
            .fetch_all(&mut **tx)
            .await
            .map_err(|_| LifecycleError::ScopeManagerUnavailable)?;
        ensure_no_scope_sessions_for_managers(&manager_ids, runner, workspace_binding_id).await
    };
    match tokio::time::timeout(SCOPE_SESSION_PROBE_BUDGET, probe).await {
        Ok(result) => result,
        Err(_) => Err(LifecycleError::ScopeManagerUnavailable),
    }
}

/// Sequential observation of an already-snapshotted manager list.
///
/// Split out so the ordering, short-circuit and owner predicate can be tested
/// without a database or a running manager.
pub(crate) async fn ensure_no_scope_sessions_for_managers(
    manager_ids: &[i64],
    runner: &dyn RunnerService,
    workspace_binding_id: i64,
) -> Result<(), LifecycleError> {
    if manager_ids.is_empty() {
        return Err(LifecycleError::ScopeManagerUnavailable);
    }
    for &manager_id in manager_ids {
        let activity = runner
            .workspace_activity_for_manager(manager_id, workspace_binding_id)
            .await
            .map_err(|_| LifecycleError::ScopeManagerUnavailable)?;
        if activity.workspace_binding_id != workspace_binding_id {
            return Err(LifecycleError::ScopeManagerUnavailable);
        }
        if activity.is_busy() {
            return Err(LifecycleError::ScopeBusy);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "scope_sessions_tests.rs"]
mod tests;
