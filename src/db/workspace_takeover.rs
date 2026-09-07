//! Atomic write-lease takeover with cancel-before-commit quiescence.
//!
//! Extracted from `workspace_bindings` to keep each file cohesive and under
//! the repository's 400-line cap. This module owns the handover state
//! machine: row-locked classification (`SELECT ... FOR UPDATE`), the
//! bounded cancellation wait, and the commit gate. Basic binding CRUD and
//! the fenced-write lock stay in `workspace_bindings`; callers keep using
//! `queries::` re-exports so no handler changes are needed.
//!
//! `force` bypasses only the staleness check; the `expected_owner`
//! compare-and-swap is always enforced. Foreign handovers run the caller
//! cancellation (session kills plus Docker container removal via
//! `cleanup_runner_owner`) while the row lock is held and commit only on
//! quiescence success; failure rolls back with `QuiescenceFailed` so the
//! identical request is safe to retry and the new run must not resume.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::{future::Future, time::Duration};

use crate::db::types::{
    TakeoverConflictReason, WorkspaceBindingResponse, WorkspaceLeaseCancellationOutcome,
    WorkspaceLeaseStatusResponse, WorkspaceLeaseStatusRow, WorkspaceLeaseTakeoverLockRow,
};

/// Outcome of an atomic, stale-guarded write-lease takeover attempt.
#[derive(Debug)]
pub enum TakeoverOutcome {
    /// The takeover (or same-owner idempotent renew) committed.
    Success {
        /// Post-update binding row from the UPDATE ... RETURNING.
        binding: WorkspaceBindingResponse,
        /// Pre-update lease owner (None when the binding had no lease).
        previous_owner: Option<String>,
        /// Pre-update `write_lease_acquired_at`.
        previous_acquired_at: Option<DateTime<Utc>>,
        /// Pre-update `write_lease_expires_at`.
        previous_expires_at: Option<DateTime<Utc>>,
        /// True when a foreign lease was displaced; false for same-owner
        /// idempotent renews.
        took_over_foreign: bool,
        /// Cancellation/quiescence outcome for the displaced binding
        /// sessions. Present (and successful) on every foreign takeover;
        /// default (not attempted) on same-owner renews.
        cancellation: WorkspaceLeaseCancellationOutcome,
    },
    /// The takeover was rejected with a machine-readable reason. The
    /// binding row is included so callers can drive a retry strategy.
    Conflict {
        reason: TakeoverConflictReason,
        current: WorkspaceLeaseStatusRow,
    },
    /// A foreign lease was eligible, but binding-scoped runner sessions
    /// could not be cancelled and verified gone. No lease change was
    /// committed: the row still reflects the previous owner, so the caller
    /// must not resume the new run and may safely retry the identical
    /// takeover request once old execution drains.
    QuiescenceFailed {
        current: WorkspaceLeaseStatusRow,
        cancellation: WorkspaceLeaseCancellationOutcome,
    },
    /// The binding does not exist.
    NotFound,
}

/// Server-controlled bound for the cancellation/quiescence wait that runs
/// while the takeover transaction holds the binding row lock.
///
/// The runner broker waits up to `RUNNER_JOB_TIMEOUT_SECS` (3660s) for a
/// manager result; without this bound a slow or disconnected runner
/// manager would pin the row for about an hour and block every fenced
/// write (shell, write_stdin, apply_patch, git-sync) for the binding.
/// Thirty seconds comfortably covers a healthy local group kill and Docker
/// container removal (milliseconds to seconds) while letting the takeover
/// roll back deterministically instead of wedging writers.
pub const TAKEOVER_CANCELLATION_TIMEOUT: Duration = Duration::from_secs(30);

/// Await the takeover cancellation future under
/// [`TAKEOVER_CANCELLATION_TIMEOUT`].
///
/// Success and logical failure pass through unchanged. A timeout maps to a
/// failed [`WorkspaceLeaseCancellationOutcome`] (attempted, not succeeded)
/// so the caller rolls back with `QuiescenceFailed` and the identical
/// request is safe to retry once old execution drains. The `cancel` future
/// is dropped on timeout, releasing the row lock with the transaction
/// rollback.
pub async fn await_takeover_cancellation(
    cancel: impl Future<Output = WorkspaceLeaseCancellationOutcome>,
) -> WorkspaceLeaseCancellationOutcome {
    await_takeover_cancellation_with_timeout(cancel, TAKEOVER_CANCELLATION_TIMEOUT).await
}

pub(crate) async fn await_takeover_cancellation_with_timeout(
    cancel: impl Future<Output = WorkspaceLeaseCancellationOutcome>,
    timeout_duration: Duration,
) -> WorkspaceLeaseCancellationOutcome {
    match tokio::time::timeout(timeout_duration, cancel).await {
        Ok(outcome) => outcome,
        Err(_) => WorkspaceLeaseCancellationOutcome {
            attempted: true,
            succeeded: false,
            sessions_cancelled: 0,
            error: Some(format!(
                "timed out after {}s waiting for runner cancellation; no lease change was committed",
                timeout_duration.as_secs()
            )),
        },
    }
}

/// Atomic, stale-guarded write-lease takeover with explicit force support.
///
/// Uses an explicit Postgres transaction with `SELECT ... FOR UPDATE`
/// to lock the active binding row before classification, then runs the
/// conditional UPDATE that reassigns the lease. The lock closes the
/// read-then-update TOCTOU window that a single-statement CTE-based
/// approach cannot close against concurrent acquire/renew/takeover,
/// because Postgres `EvalPlanQual` does not refresh CTE snapshot
/// values.
///
/// The stale guard uses the database clock (`db_now` returned by the
/// lock query) rather than the application clock, so the threshold is
/// evaluated against the same time source as the existing acquire and
/// renew statements.
///
/// `force` is a backward-compatible opt-in for user-confirmed takeover of
/// a live lease. It bypasses only the staleness check; the
/// `expected_owner` compare-and-swap is still enforced. Callers must set
/// `force` solely from explicit user confirmation.
///
/// Quiescence is part of the handover, not a post-commit side effect: on
/// a foreign takeover the caller-supplied `cancel` future runs while the
/// row lock is still held, and the lease UPDATE commits only when
/// cancellation/quiescence succeeded (see [`takeover_commit_allowed`]).
/// A failed cancellation rolls back without transferring the lease, so
/// the caller must not resume the new run and may safely retry the
/// identical request. Same-owner renews never run `cancel`.
///
/// Bounded cancellation: the `cancel` future runs under
/// [`TAKEOVER_CANCELLATION_TIMEOUT`] while the row lock is held. Without
/// a bound a slow or unavailable runner manager pins the binding row for
/// up to `RUNNER_JOB_TIMEOUT_SECS` (3660s), blocking every fenced write
/// for the binding. A timeout maps to `QuiescenceFailed` with no lease
/// change, so the identical request is safe to retry.
///
/// Classification:
///   * `new_owner` already holds the lease      -> idempotent renew,
///     `took_over_foreign = false`, no session cancellation.
///   * `expected_owner` holds the lease and (`force` is set or the
///     last refresh is at least `stale_threshold_seconds` old) -> foreign
///     takeover, `took_over_foreign = true`. Binding-scoped runner
///     sessions are cancelled and verified gone while the row lock is
///     held; cancellation/quiescence failure returns `QuiescenceFailed`
///     with no lease change.
///   * Otherwise the request returns a structured `Conflict` with a
///     machine-readable reason.
#[allow(clippy::too_many_arguments)]
pub async fn acquire_workspace_write_lease_takeover(
    pool: &PgPool,
    workspace_binding_id: i64,
    new_owner: &str,
    ttl_seconds: u64,
    expected_owner: &str,
    stale_threshold_seconds: u64,
    force: bool,
    cancel: impl Future<Output = WorkspaceLeaseCancellationOutcome>,
) -> anyhow::Result<TakeoverOutcome> {
    // Pre-transaction probe: distinguishes "binding does not exist"
    // (404) from "binding exists but is not active" (409 with
    // reason=not_active). The probe is non-locking; the authoritative
    // state is locked below.
    let probe: Option<WorkspaceLeaseStatusRow> = sqlx::query_as::<_, WorkspaceLeaseStatusRow>(
        include_str!("../sql/read_workspace_write_lease_for_takeover.sql"),
    )
    .bind(workspace_binding_id)
    .fetch_optional(pool)
    .await?;
    let Some(probe) = probe else {
        return Ok(TakeoverOutcome::NotFound);
    };
    if !probe.is_active || probe.lifecycle_state != "active" {
        return Ok(TakeoverOutcome::Conflict {
            reason: TakeoverConflictReason::NotActive,
            current: probe,
        });
    }

    let mut tx = pool.begin().await?;

    // Lock the active binding row. The `FOR UPDATE` clause serializes
    // concurrent acquire/renew/takeover against this binding so no
    // concurrent writer can refresh the lease between our staleness
    // decision and the assignment UPDATE.
    let locked: Option<WorkspaceLeaseTakeoverLockRow> =
        sqlx::query_as::<_, WorkspaceLeaseTakeoverLockRow>(include_str!(
            "../sql/lock_workspace_write_lease_for_takeover.sql"
        ))
        .bind(workspace_binding_id)
        .fetch_optional(&mut *tx)
        .await?;

    let Some(locked) = locked else {
        // Race: the binding was deactivated between the probe and the
        // lock. Surface the latest known state as a NotActive conflict.
        tx.rollback().await?;
        return Ok(TakeoverOutcome::Conflict {
            reason: TakeoverConflictReason::NotActive,
            current: probe,
        });
    };

    let decision = classify_takeover(
        &locked,
        new_owner,
        expected_owner,
        stale_threshold_seconds,
        force,
    );
    match decision {
        TakeoverDecision::Update { took_over_foreign } => {
            // Foreign handovers verify quiescence while the row lock is
            // still held: fenced writers admitted before this takeover
            // block on the lock instead of racing the check, and no new
            // fenced write can dispatch between verification and commit.
            // A failed verification rolls back with no lease change, so
            // the identical request is safe to retry. The wait is bounded
            // so the row lock is never pinned for the full runner job
            // timeout (see `await_takeover_cancellation`).
            let cancellation = if took_over_foreign {
                await_takeover_cancellation(cancel).await
            } else {
                WorkspaceLeaseCancellationOutcome::default()
            };
            if !takeover_commit_allowed(took_over_foreign, cancellation.succeeded) {
                tx.rollback().await?;
                let current: WorkspaceLeaseStatusRow = WorkspaceLeaseStatusRow {
                    workspace_binding_id: locked.workspace_binding_id,
                    application_id: locked.application_id,
                    workspace_key: locked.workspace_key,
                    is_active: locked.is_active,
                    lifecycle_state: locked.lifecycle_state,
                    resource_kind: locked.resource_kind,
                    resource_id: locked.resource_id,
                    write_lease_owner: locked.write_lease_owner,
                    write_lease_acquired_at: locked.write_lease_acquired_at,
                    write_lease_expires_at: locked.write_lease_expires_at,
                    db_now: locked.db_now,
                };
                return Ok(TakeoverOutcome::QuiescenceFailed {
                    current,
                    cancellation,
                });
            }
            let binding: WorkspaceBindingResponse = sqlx::query_as::<_, WorkspaceBindingResponse>(
                include_str!("../sql/acquire_workspace_write_lease_takeover.sql"),
            )
            .bind(new_owner)
            .bind(i64::try_from(ttl_seconds).unwrap_or(i64::MAX))
            .bind(workspace_binding_id)
            .fetch_one(&mut *tx)
            .await?;
            tx.commit().await?;
            Ok(TakeoverOutcome::Success {
                binding,
                previous_owner: locked.write_lease_owner,
                previous_acquired_at: locked.write_lease_acquired_at,
                previous_expires_at: locked.write_lease_expires_at,
                took_over_foreign,
                cancellation,
            })
        }
        TakeoverDecision::Conflict(reason) => {
            tx.rollback().await?;
            let current: WorkspaceLeaseStatusRow = WorkspaceLeaseStatusRow {
                workspace_binding_id: locked.workspace_binding_id,
                application_id: locked.application_id,
                workspace_key: locked.workspace_key,
                is_active: locked.is_active,
                lifecycle_state: locked.lifecycle_state,
                resource_kind: locked.resource_kind,
                resource_id: locked.resource_id,
                write_lease_owner: locked.write_lease_owner,
                write_lease_acquired_at: locked.write_lease_acquired_at,
                write_lease_expires_at: locked.write_lease_expires_at,
                db_now: locked.db_now,
            };
            Ok(TakeoverOutcome::Conflict { reason, current })
        }
    }
}

#[derive(Debug)]
pub(crate) enum TakeoverDecision {
    Update { took_over_foreign: bool },
    Conflict(TakeoverConflictReason),
}

/// Pure classifier for the takeover decision. Kept as a free function so
/// the stale timestamp semantics can be unit-tested without a database.
///
/// `force` bypasses only the staleness check for a foreign lease that
/// matches `expected_owner`. The CAS itself is never bypassed: a force
/// request with a mismatched `expected_owner`, with no lease, or against
/// an inactive binding still returns the corresponding conflict.
pub(crate) fn classify_takeover(
    locked: &WorkspaceLeaseTakeoverLockRow,
    new_owner: &str,
    expected_owner: &str,
    stale_threshold_seconds: u64,
    force: bool,
) -> TakeoverDecision {
    let lease_owner = locked.write_lease_owner.as_deref();
    let is_stale = locked
        .write_lease_acquired_at
        .is_some_and(|acquired| lease_is_stale(locked.db_now, acquired, stale_threshold_seconds));

    if lease_owner == Some(new_owner) {
        // Idempotent same-owner renew. Session cancellation must NOT
        // run on this path.
        TakeoverDecision::Update {
            took_over_foreign: false,
        }
    } else if lease_owner == Some(expected_owner) && (force || is_stale) {
        // Foreign takeover: the lease matches the caller's expected
        // owner and either has not been refreshed within the stale window
        // or the caller explicitly confirmed a live (force) takeover.
        TakeoverDecision::Update {
            took_over_foreign: true,
        }
    } else if lease_owner.is_none() {
        TakeoverDecision::Conflict(TakeoverConflictReason::NoLease)
    } else if lease_owner == Some(expected_owner) {
        // The lease matches expected_owner but is still inside the
        // stale window and force was not set; caller should wait or retry
        // with an explicit user-confirmed force request.
        TakeoverDecision::Conflict(TakeoverConflictReason::LiveLease)
    } else {
        // The lease owner differs from expected_owner (and from
        // new_owner); the writer that refreshed the lease has changed.
        TakeoverDecision::Conflict(TakeoverConflictReason::ExpectedOwnerMismatch)
    }
}

/// Pure commit gate for the takeover handover.
///
/// A foreign takeover may commit only when binding-scoped runner
/// cancellation/quiescence succeeded. Same-owner renews never displace
/// another writer, so they are unaffected by the cancellation outcome.
/// The takeover transaction consults this gate while still holding the
/// row lock: `false` rolls back with no lease change (`QuiescenceFailed`)
/// instead of leaving a transferred lease alongside running old
/// execution.
pub(crate) fn takeover_commit_allowed(
    took_over_foreign: bool,
    cancellation_succeeded: bool,
) -> bool {
    !took_over_foreign || cancellation_succeeded
}

/// True when the lease's last refresh timestamp is at least
/// `threshold_seconds` older than the supplied database clock. Uses
/// `chrono`'s duration arithmetic against UTC and clamps negative elapsed
/// times to zero (clock-skew defense).
pub(crate) fn lease_is_stale(
    db_now: DateTime<Utc>,
    acquired_at: DateTime<Utc>,
    threshold_seconds: u64,
) -> bool {
    let elapsed_seconds = (db_now - acquired_at).num_seconds().max(0);
    let threshold = i64::try_from(threshold_seconds).unwrap_or(i64::MAX);
    elapsed_seconds >= threshold
}

/// Convert a lease row into the response shared by the status and takeover
/// conflict endpoints. The row and threshold use the same database clock
/// semantics as takeover classification.
pub(crate) fn workspace_lease_status_response(
    row: WorkspaceLeaseStatusRow,
    stale_threshold_seconds: u64,
) -> WorkspaceLeaseStatusResponse {
    let stale = row.write_lease_owner.is_some()
        && row.write_lease_acquired_at.is_some_and(|acquired_at| {
            lease_is_stale(row.db_now, acquired_at, stale_threshold_seconds)
        });

    WorkspaceLeaseStatusResponse {
        workspace_binding_id: row.workspace_binding_id,
        application_id: row.application_id,
        workspace_key: row.workspace_key,
        is_active: row.is_active,
        lifecycle_state: row.lifecycle_state,
        resource_kind: row.resource_kind,
        resource_id: row.resource_id,
        write_lease_owner: row.write_lease_owner,
        write_lease_acquired_at: row.write_lease_acquired_at,
        write_lease_expires_at: row.write_lease_expires_at,
        stale,
        stale_threshold_seconds,
    }
}
