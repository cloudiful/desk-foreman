//! Abandoned broker-job skip for takeover timeouts.
//!
//! When the takeover outer timeout drops the cancellation future, the
//! in-flight `submit_inner` future is dropped mid-await: its oneshot
//! `receiver` is dropped, but the `QueuedJob` and `PendingJob` entries stay
//! in the broker maps. Without a skip the manager later dispatches the
//! abandoned job and runs cancel/container-stop side effects after the
//! transaction already rolled back (lease unchanged).
//!
//! Retry safety of late work (why skipping queued jobs is sufficient
//! without a broad protocol redesign):
//!   * Session cancels target old `session_id`s. New sessions after a retry
//!     have different ids, so a late cancel of an old id is either a
//!     desired cleanup of leftover old execution or a harmless unknown-id
//!     error. It cannot kill a new owner's session.
//!   * Container stops (`cleanup_runner_owner`) check active-operation
//!     counts on the runner manager and bail while new sessions hold
//!     leases, instead of killing the fresh container. An empty-container
//!     removal is idempotent churn (bind-mounted workspace files survive;
//!     the next `ensure_runner` recreates).
//! Still, queued-not-yet-dispatched abandoned jobs are skipped via
//! `sender.is_closed()` (true once the receiver is dropped) to avoid
//! confusing UX (old sessions cancelled by a takeover reported as failed)
//! and wasted daemon work. Already-dispatched jobs cannot be un-run; they
//! complete and their `complete_job` send is ignored on the closed sender.
//! If stronger per-takeover abort tokens are ever required, return an
//! explicit blocker instead of bluffing safety.

/// True when a queued (not yet dispatched) broker job was abandoned by its
/// caller and must be skipped before dispatch.
///
/// `sender_closed` is `pending.sender.is_closed()` (true after the outer
/// takeover timeout drops the receiver). `already_dispatched` is
/// `pending.manager_id.is_some()`. Dispatched jobs are left to complete
/// (their send is ignored); only queued jobs are pruned.
pub(crate) fn should_skip_abandoned_queued_job(
    sender_closed: bool,
    already_dispatched: bool,
) -> bool {
    sender_closed && !already_dispatched
}

/// Filter helper for tests and `next_job`: given parallel slices of
/// `(job_id, sender_closed, dispatched)`, return the ids to prune.
#[cfg(test)]
pub(crate) fn abandoned_queued_job_ids(jobs: &[(String, bool, bool)]) -> Vec<String> {
    jobs.iter()
        .filter(|(_, closed, dispatched)| should_skip_abandoned_queued_job(*closed, *dispatched))
        .map(|(id, _, _)| id.clone())
        .collect()
}
