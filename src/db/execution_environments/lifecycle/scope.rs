//! Scoped provision/replace authority, guards and manager affinity
//! (issue 1180).
//!
//! Scoped lifecycle admission is only reachable through an authenticated scope
//! authority and a fresh bound write lease. Everything here runs inside the
//! caller's existing lifecycle transaction under the environment row lock; the
//! binding row lock is taken here and held to commit, including one bounded
//! out-of-poll session observation whose absolute budget caps the contention
//! window. No Docker work happens under either lock.

use chrono::{DateTime, Utc};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::db::types::{ExecutionEnvironment, WorkspaceBindingResponse};
use crate::db::workspace_bindings::native_task_guard::native_task_holds_binding;
use crate::db::workspace_bindings::scope_admission::validate_scope_binding;
use crate::db::workspace_bindings::workspace_scope_guard::workspace_scope_holds_binding;
use crate::runner::RunnerService;

use super::LifecycleError;
use super::scope_sessions::ensure_no_scope_sessions;
use super::types::ScopeAuthority;

/// Fresh bound identity, root and validated lease expiry of an authorized
/// workspace scope.
pub(crate) struct ScopedBinding {
    pub(crate) workspace_binding_id: i64,
    pub(crate) workspace_root: String,
    pub(crate) lease_expires_at: DateTime<Utc>,
}

/// One potential-writer manager group for the bound workspace.
///
/// `authorizes` is the grouped positive-admission boolean: true only when every
/// observation in the group is running, freshly observed and backed by an
/// enabled/online/fresh manager. A stale, offline, disabled, unknown or
/// orphaned group is retained and blocks without authorizing.
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct LiveScopeManager {
    pub(crate) runner_manager_id: Option<i64>,
    pub(crate) workspace_root: String,
    pub(crate) authorizes: bool,
}

/// Read the stored scope and, when present, lock and validate the fresh bound
/// binding under the caller's transaction.
///
/// The stored-scope read reuses the checked row projection rather than
/// widening the lifecycle row. A stored scope requires an authority with the
/// exact same binding and an explicit lease that the shared validator accepts
/// against the locked row; anything else fails closed before replay or intent.
/// An unscoped row is returned as `None` and keeps the legacy path.
pub(crate) async fn authorize_scope(
    tx: &mut Transaction<'_, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    authority: Option<&ScopeAuthority>,
) -> Result<Option<ScopedBinding>, LifecycleError> {
    let stored: Option<ExecutionEnvironment> = sqlx::query_file_as!(
        ExecutionEnvironment,
        "src/sql/execution_environments/find_execution_environment_by_id.sql",
        environment_id,
        application_id,
    )
    .fetch_optional(&mut **tx)
    .await?;
    let stored = stored.ok_or_else(|| {
        LifecycleError::Internal(anyhow::anyhow!(
            "execution environment row is unavailable inside its lifecycle transaction"
        ))
    })?;
    let Some(workspace_binding_id) = stored.workspace_binding_id else {
        return Ok(None);
    };

    let authority = authority.ok_or(LifecycleError::ScopeAuthorityRequired)?;
    if authority.workspace_binding_id != workspace_binding_id {
        return Err(LifecycleError::ScopeIneligible);
    }

    // The binding row lock is held to commit by the caller's transaction, so
    // the lease checked here cannot be taken over before the intent is
    // inserted.
    let fresh: Option<WorkspaceBindingResponse> = sqlx::query_as::<_, WorkspaceBindingResponse>(
        include_str!("../../../sql/lock_workspace_binding_for_write.sql"),
    )
    .bind(workspace_binding_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(fresh) = fresh else {
        return Err(LifecycleError::ScopeIneligible);
    };
    validate_scope_binding(
        &fresh,
        application_id,
        authority.workspace_binding_id,
        Some(authority.lease_owner.as_str()),
    )
    .map_err(|_| LifecycleError::ScopeIneligible)?;
    let Some(lease_expires_at) = fresh.write_lease_expires_at else {
        return Err(LifecycleError::ScopeIneligible);
    };

    Ok(Some(ScopedBinding {
        workspace_binding_id,
        workspace_root: fresh.workspace_root,
        lease_expires_at,
    }))
}

/// Run the fresh binding-level guards, resolve the single usable manager
/// observation and prove no binding session is live before a scoped new
/// intent.
///
/// The caller must have already proven the environment's own state absent or
/// replaced, so its own anchor never blocks here. No select-manager fallback
/// exists on the scoped path. The bounded session observation runs under the
/// same held binding fence; the lease is rechecked after every probe so a
/// lease that expired during the observation cannot authorize the intent.
pub(crate) async fn admit_scoped_intent(
    tx: &mut Transaction<'_, Postgres>,
    binding: &ScopedBinding,
    assigned_manager_id: Option<i64>,
    runner: Option<&dyn RunnerService>,
) -> Result<i64, LifecycleError> {
    if native_task_holds_binding(tx, binding.workspace_binding_id)
        .await?
        .is_some()
    {
        return Err(LifecycleError::NativeTaskActive);
    }
    if workspace_scope_holds_binding(tx, binding.workspace_binding_id)
        .await?
        .is_some()
    {
        return Err(LifecycleError::ScopeBusy);
    }
    let observations = find_live_scope_managers(tx, binding.workspace_binding_id).await?;
    let manager_id =
        select_scope_manager(&observations, &binding.workspace_root, assigned_manager_id)?;

    let Some(runner) = runner else {
        return Err(LifecycleError::ScopeManagerUnavailable);
    };
    ensure_no_scope_sessions(tx, runner, binding.workspace_binding_id).await?;
    if binding.lease_expires_at <= Utc::now() {
        return Err(LifecycleError::ScopeIneligible);
    }
    Ok(manager_id)
}

async fn find_live_scope_managers(
    tx: &mut Transaction<'_, Postgres>,
    workspace_binding_id: i64,
) -> anyhow::Result<Vec<LiveScopeManager>> {
    Ok(sqlx::query_file_as!(
        LiveScopeManager,
        "src/sql/find_live_scope_managers.sql",
        workspace_binding_id,
        runner_protocol::RUNNER_MANAGER_HEARTBEAT_TTL_SECS as i64,
    )
    .fetch_all(&mut **tx)
    .await?)
}

/// Pick the one potential-writer group that may authorize a scoped intent.
///
/// Exactly one group is required and it must authorize. A group whose manager
/// id is absent or nonpositive, whose root does not byte-equal the freshly
/// locked binding root, or whose manager differs from an existing assignment
/// never authorizes. Every retained non-authorizing group still rejects by
/// making the observation set ambiguous; there is no fallback.
pub(crate) fn select_scope_manager(
    observations: &[LiveScopeManager],
    expected_root: &str,
    assigned_manager_id: Option<i64>,
) -> Result<i64, LifecycleError> {
    let [observation] = observations else {
        return Err(LifecycleError::ScopeManagerUnavailable);
    };
    if !observation.authorizes {
        return Err(LifecycleError::ScopeManagerUnavailable);
    }
    if observation.workspace_root != expected_root {
        return Err(LifecycleError::ScopeManagerUnavailable);
    }
    let Some(manager_id) = observation
        .runner_manager_id
        .filter(|manager_id| *manager_id > 0)
    else {
        return Err(LifecycleError::ScopeManagerUnavailable);
    };
    if let Some(assigned_manager_id) = assigned_manager_id {
        if assigned_manager_id != manager_id {
            return Err(LifecycleError::ScopeManagerUnavailable);
        }
    }
    Ok(manager_id)
}

#[cfg(test)]
mod tests {
    use super::{LiveScopeManager, select_scope_manager};
    use crate::db::execution_environments::lifecycle::LifecycleError;

    const ROOT: &str = "/workspace/fixture";

    fn observation(manager_id: Option<i64>, root: &str, authorizes: bool) -> LiveScopeManager {
        LiveScopeManager {
            runner_manager_id: manager_id,
            workspace_root: root.to_string(),
            authorizes,
        }
    }

    #[test]
    fn scope_manager_selection_table() {
        let cases: [(&str, Vec<LiveScopeManager>, Option<i64>, Result<i64, ()>); 11] = [
            ("no observation", Vec::new(), None, Err(())),
            (
                "one authorizing observation",
                vec![observation(Some(3), ROOT, true)],
                None,
                Ok(3),
            ),
            (
                "two authorizing observations",
                vec![
                    observation(Some(3), ROOT, true),
                    observation(Some(4), ROOT, true),
                ],
                None,
                Err(()),
            ),
            (
                "root mismatch",
                vec![observation(Some(3), "/workspace/other", true)],
                None,
                Err(()),
            ),
            (
                "assigned manager mismatch",
                vec![observation(Some(3), ROOT, true)],
                Some(4),
                Err(()),
            ),
            (
                "assigned manager matches",
                vec![observation(Some(3), ROOT, true)],
                Some(3),
                Ok(3),
            ),
            (
                "non-authorizing singleton",
                vec![observation(Some(3), ROOT, false)],
                None,
                Err(()),
            ),
            (
                "orphan observation",
                vec![observation(None, ROOT, false)],
                None,
                Err(()),
            ),
            (
                "authorizing orphan without a manager",
                vec![observation(None, ROOT, true)],
                None,
                Err(()),
            ),
            (
                "valid plus unknown pair",
                vec![
                    observation(Some(3), ROOT, true),
                    observation(Some(4), ROOT, false),
                ],
                None,
                Err(()),
            ),
            (
                "nonpositive manager",
                vec![observation(Some(0), ROOT, true)],
                None,
                Err(()),
            ),
        ];
        for (name, observations, assigned, expected) in cases {
            let result = select_scope_manager(&observations, ROOT, assigned)
                .map_err(|error| assert!(matches!(error, LifecycleError::ScopeManagerUnavailable)));
            assert_eq!(
                result.map_err(|_| ()),
                expected,
                "unexpected selection for {name}"
            );
        }
    }
}
