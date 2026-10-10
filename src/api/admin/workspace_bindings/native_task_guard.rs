//! Native-task guard for admin workspace-binding transitions (issue 753 P1).
//!
//! Archive, restore and reset all change what a workspace *is*: they archive
//! it, or they destroy and re-create its directory. A durable native task
//! holds that workspace for as long as it is active, so each of those must
//! refuse while one does — an admitted task can never race a directory
//! destruction it did not know about.
//!
//! Every helper here runs inside the caller's already-open transaction that
//! holds the binding row lock, and re-reads the guard after that lock.

use sqlx::{PgPool, Postgres, Transaction};

use crate::db::workspace_bindings::native_task_guard::native_task_holds_binding;

// Re-exported so the handler reaches the guarded state transition through
// this module's name: one guard module, one way to mutate a guarded binding.
pub(crate) use crate::db::workspace_bindings::native_task_guard::set_lifecycle_state;

/// A binding locked for a lifecycle transition, and why it may not proceed.
///
/// The transaction travels with the guard so the caller cannot drop the row
/// lock before the filesystem work and the commit: reset in particular must
/// hold it across directory destruction.
pub(crate) enum TransitionGuard<'a> {
    /// The binding exists and no native task holds it.
    Ready(Transaction<'a, Postgres>),
    /// No such binding for this id.
    Absent(Transaction<'a, Postgres>),
    /// A durable native task owns this workspace. The caller must refuse.
    NativeTaskHolds(Transaction<'a, Postgres>),
}

impl<'a> TransitionGuard<'a> {
    /// True when no such binding exists.
    pub(crate) fn is_absent(&self) -> bool {
        matches!(self, Self::Absent(_))
    }

    /// True when a native task owns this workspace.
    pub(crate) fn is_native_task_conflict(&self) -> bool {
        matches!(self, Self::NativeTaskHolds(_))
    }

    /// The guarded transaction itself.
    ///
    /// The state transition must run on this transaction: a second connection
    /// would move the change outside the window the row lock closes.
    pub(crate) fn transaction_mut(&mut self) -> &mut Transaction<'a, Postgres> {
        match self {
            Self::Ready(tx) | Self::Absent(tx) | Self::NativeTaskHolds(tx) => tx,
        }
    }

    /// Commit the guarded transaction, releasing the binding row lock.
    pub(crate) async fn commit(self) -> anyhow::Result<()> {
        match self {
            Self::Ready(tx) | Self::Absent(tx) | Self::NativeTaskHolds(tx) => {
                tx.commit().await?;
                Ok(())
            }
        }
    }
}

/// Lock one binding row for a transition and check the native-task guard.
///
/// The row lock is what makes the guard meaningful: task admission takes the
/// same lock, so either the task committed before us (and we see it here) or
/// it waits for us (and sees our transition). There is no interleaving where
/// both believe they own the workspace.
pub(crate) async fn lock_binding_for_transition(
    pool: &PgPool,
    binding_id: i64,
) -> anyhow::Result<TransitionGuard<'_>> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query_file_scalar!(
        "src/sql/workspace_bindings/lock_workspace_binding_for_transition.sql",
        binding_id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    if row.is_none() {
        return Ok(TransitionGuard::Absent(tx));
    }
    if native_task_holds_binding(&mut tx, binding_id)
        .await?
        .is_some()
    {
        return Ok(TransitionGuard::NativeTaskHolds(tx));
    }
    Ok(TransitionGuard::Ready(tx))
}
