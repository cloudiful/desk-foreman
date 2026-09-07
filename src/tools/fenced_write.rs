//! Transaction-scoped write fence for mutating tools.
//!
//! Extracted from `shared.rs` to keep each file under the 400-line cap.
//! Re-exported via `crate::tools::shared::{FencedWrite, admit_workspace_write}`
//! so existing callers keep working.

use crate::{
    AppState,
    actor::{ActorContext, admit_fenced_write},
    tools::common::{ToolError, tool_internal},
};

/// Transaction-scoped write fence for mutating tools.
///
/// Admission locks the binding row
/// ([`crate::db::workspace_bindings::lock_workspace_binding_for_write`])
/// and checks the locked fresh row instead of the authentication-time
/// snapshot. The guard keeps the row lock held through the write dispatch
/// and commits afterwards, so a concurrent lease takeover (which locks the
/// same row) serializes against the write: an already-admitted old-owner
/// write either dispatches before the handover or fails its locked check
/// after it, but never crosses it. This covers direct workspace-SDK file
/// operations such as `apply_patch` as well as runner-backed writes.
/// Dropping the guard without [`FencedWrite::commit`] rolls the lock back.
///
/// Database failures surface as internal errors so they are never confused
/// with a lease denial.
pub struct FencedWrite<'a> {
    pub(crate) tx: Option<sqlx::Transaction<'a, sqlx::Postgres>>,
}

/// Admit a mutating tool call under the write fence.
///
/// Per-user workspaces (and actors without a binding) need no fence and
/// return an empty guard without touching the database. Resource-owned
/// workspaces lock the binding row and validate the locked fresh row via
/// [`admit_fenced_write`]. Hold the returned guard across the write
/// dispatch and [`FencedWrite::commit`] it afterwards.
pub async fn admit_workspace_write<'a>(
    state: &'a AppState,
    actor: &'a ActorContext,
) -> Result<FencedWrite<'a>, ToolError> {
    let Some(snapshot) = &actor.workspace_binding else {
        return Ok(FencedWrite { tx: None });
    };
    if snapshot.resource_kind.is_none() {
        return Ok(FencedWrite { tx: None });
    }
    let (tx, fresh) = crate::db::queries::lock_workspace_binding_for_write(
        &state.db,
        snapshot.workspace_binding_id,
    )
    .await
    .map_err(tool_internal)?;
    admit_fenced_write(fresh.as_ref(), actor.lease_owner.as_deref())
        .map_err(ToolError::Forbidden)?;
    Ok(FencedWrite { tx: Some(tx) })
}

impl FencedWrite<'_> {
    /// Release the row lock after the write dispatched.
    pub async fn commit(self) -> Result<(), ToolError> {
        if let Some(tx) = self.tx {
            tx.commit().await.map_err(tool_internal)?;
        }
        Ok(())
    }
}
