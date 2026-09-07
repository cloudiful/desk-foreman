//! Unit regression for abandoned broker-job skip (no live DB).
//!
//! Uses real tokio oneshot channels so `sender.is_closed()` transitions are
//! genuine, plus a simulated dispatch queue that applies the same predicate
//! `next_job` uses. Proves an outer-timeout drop (receiver dropped) is
//! observable as closed and that queued abandoned jobs are skipped while
//! live and already-dispatched jobs are kept.

use super::pull_cancellation::{abandoned_queued_job_ids, should_skip_abandoned_queued_job};

#[test]
fn skip_predicate_keeps_live_and_dispatched_jobs() {
    assert!(!should_skip_abandoned_queued_job(false, false));
    assert!(should_skip_abandoned_queued_job(true, false));
    // Already dispatched: let it complete (send ignored), don't prune early.
    assert!(!should_skip_abandoned_queued_job(true, true));
    assert!(!should_skip_abandoned_queued_job(false, true));
}

#[test]
fn receiver_drop_is_observable_as_abandoned() {
    let (sender, receiver) = tokio::sync::oneshot::channel::<anyhow::Result<serde_json::Value>>();
    assert!(!sender.is_closed());
    drop(receiver);
    assert!(
        sender.is_closed(),
        "dropping the takeover future's receiver must read as abandoned"
    );
}

#[test]
fn simulated_queue_dispatch_skips_abandoned_queued_jobs() {
    // (job_id, sender_closed, dispatched)
    let queue = vec![
        ("live-1".to_string(), false, false),
        ("abandoned-1".to_string(), true, false),
        ("dispatched-live".to_string(), false, true),
        ("dispatched-abandoned".to_string(), true, true),
        ("live-2".to_string(), false, false),
    ];
    let pruned = abandoned_queued_job_ids(&queue);
    assert_eq!(pruned, vec!["abandoned-1".to_string()]);

    // Dispatch selection over the pruned queue picks the first live queued job.
    let remaining: Vec<&str> = queue
        .iter()
        .filter(|(id, _, _)| !pruned.contains(id))
        .filter(|(_, _, dispatched)| !dispatched)
        .map(|(id, _, _)| id.as_str())
        .collect();
    assert_eq!(remaining, vec!["live-1", "live-2"]);
}

#[tokio::test]
async fn real_oneshot_abandonment_drives_queue_skip() {
    let (live_sender, _live_receiver) =
        tokio::sync::oneshot::channel::<anyhow::Result<serde_json::Value>>();
    let (dead_sender, dead_receiver) =
        tokio::sync::oneshot::channel::<anyhow::Result<serde_json::Value>>();
    drop(dead_receiver);

    let queue = vec![
        ("live".to_string(), live_sender.is_closed(), false),
        ("dead".to_string(), dead_sender.is_closed(), false),
    ];
    let pruned = abandoned_queued_job_ids(&queue);
    assert_eq!(pruned, vec!["dead".to_string()]);
}
