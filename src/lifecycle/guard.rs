//! Janitor guard for archived workspace cleanup (issue 753 P1).
//!
//! The janitor deletes archived workspace directories. A durable native task
//! owns its workspace for as long as it is active, so cleanup must take the
//! same binding row lock the admin transitions take and re-read the same
//! guard query — one rule, consulted twice, never two implementations.
//!
//! The lock is held through the filesystem cleanup and the final delete, so a
//! task admitted mid-cleanup cannot have its workspace destroyed underneath
//! it.

use sqlx::{PgPool, Postgres, Transaction};

use crate::db::workspace_bindings::native_task_guard::native_task_holds_binding;

/// The cleanup decision for one archived binding.
pub(crate) enum CleanupGuard<'a> {
    /// No native task owns this workspace: cleanup may proceed.
    Clear(Transaction<'a, Postgres>),
    /// A durable native task still owns it: skip and leave everything alone.
    Skip(Transaction<'a, Postgres>),
}

impl<'a> CleanupGuard<'a> {
    /// True when the janitor must skip this binding entirely.
    pub(crate) fn should_skip(&self) -> bool {
        matches!(self, Self::Skip(_))
    }

    /// The guarded transaction, for the delete.
    ///
    /// The row must be deleted on this transaction: a second connection would
    /// remove a row this guard still holds, outside the window it closes.
    pub(crate) fn transaction_mut(&mut self) -> &mut Transaction<'a, Postgres> {
        match self {
            Self::Clear(tx) | Self::Skip(tx) => tx,
        }
    }

    /// Release the binding row lock by committing the guarded transaction.
    pub(crate) async fn release(self) -> anyhow::Result<()> {
        match self {
            Self::Clear(tx) | Self::Skip(tx) => {
                tx.commit().await?;
                Ok(())
            }
        }
    }
}

/// Lock the binding row and re-read the native-task guard for cleanup.
pub(crate) async fn lock_binding_for_cleanup(
    pool: &PgPool,
    binding_id: i64,
) -> anyhow::Result<CleanupGuard<'_>> {
    let mut tx = pool.begin().await?;
    let locked = sqlx::query_file_scalar!(
        "src/sql/workspace_bindings/lock_workspace_binding_for_cleanup.sql",
        binding_id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    if locked.is_none() {
        // Already gone: nothing to clean, and the lock is released by commit.
        return Ok(CleanupGuard::Clear(tx));
    }
    if native_task_holds_binding(&mut tx, binding_id)
        .await?
        .is_some()
    {
        return Ok(CleanupGuard::Skip(tx));
    }
    Ok(CleanupGuard::Clear(tx))
}
