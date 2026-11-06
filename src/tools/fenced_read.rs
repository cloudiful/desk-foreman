//! Transaction-scoped read fence for container-creating read tools.

use crate::{
    AppState,
    actor::{ActorContext, admit_read},
    tools::common::{ToolError, tool_internal},
};

/// Transaction-scoped fence for read tools that may create a runner container.
///
/// Bound workspaces lock the binding row and revalidate the locked fresh row
/// before dispatch. The lock is held through the runner call (including the
/// first Running acknowledgement) and committed afterwards, so a concurrent
/// native task, Git job, scoped session, archive or takeover serializes against
/// the read. Reads acquire no write lease. Dropping the guard without
/// [`FencedRead::commit`] rolls the lock back.
pub struct FencedRead<'a> {
    pub(crate) tx: Option<sqlx::Transaction<'a, sqlx::Postgres>>,
}

/// Admit a read tool call under the fresh binding row lock.
///
/// Actors without a binding (including an unbound user workspace) need no
/// fence and return an empty guard without touching the database. A bound
/// workspace locks its binding row and revalidates its identity, active
/// lifecycle and workspace root. Hold the guard across the runner dispatch and
/// [`FencedRead::commit`] it afterwards.
pub async fn admit_workspace_read<'a>(
    state: &'a AppState,
    actor: &'a ActorContext,
) -> Result<FencedRead<'a>, ToolError> {
    let Some(snapshot) = &actor.workspace_binding else {
        return Ok(FencedRead { tx: None });
    };
    let (tx, fresh) = crate::db::queries::lock_workspace_binding_for_write(
        &state.db,
        snapshot.workspace_binding_id,
    )
    .await
    .map_err(tool_internal)?;
    admit_read(fresh.as_ref(), snapshot).map_err(ToolError::Forbidden)?;
    Ok(FencedRead { tx: Some(tx) })
}

impl FencedRead<'_> {
    /// Release the row lock after the read dispatched.
    pub async fn commit(self) -> Result<(), ToolError> {
        if let Some(tx) = self.tx {
            tx.commit().await.map_err(tool_internal)?;
        }
        Ok(())
    }
}
