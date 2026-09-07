//! Takeover handover unit tests (no live DB).
//!
//! Covers the pure classifier, commit gate, stale semantics, bounded
//! cancellation, and wire-format stability for the force-takeover path.
//! Kept separate from `workspace_bindings` so neither file breaches the
//! repository's 400-line cap.

use chrono::{Duration, Utc};

use super::workspace_takeover::{
    TakeoverDecision, await_takeover_cancellation_with_timeout, classify_takeover, lease_is_stale,
    takeover_commit_allowed, workspace_lease_status_response,
};
use crate::db::types::{
    TakeoverConflictReason, WorkspaceLeaseStatusRow, WorkspaceLeaseTakeoverLockRow,
};

fn lock_row_with(lease_owner: Option<&str>, age_seconds: i64) -> WorkspaceLeaseTakeoverLockRow {
    let now = Utc::now();
    WorkspaceLeaseTakeoverLockRow {
        workspace_binding_id: 1,
        application_id: 1,
        workspace_key: "code_project:abc".to_string(),
        is_active: true,
        lifecycle_state: "active".to_string(),
        resource_kind: Some("code_project".to_string()),
        resource_id: Some("abc".to_string()),
        write_lease_owner: lease_owner.map(str::to_string),
        write_lease_acquired_at: lease_owner.map(|_| now - Duration::seconds(age_seconds)),
        write_lease_expires_at: lease_owner
            .map(|_| now - Duration::seconds(age_seconds) + Duration::seconds(600)),
        db_now: now,
    }
}

#[test]
fn lease_is_stale_boundary_matches_threshold() {
    let now = Utc::now();
    // Exactly at threshold -> stale.
    assert!(lease_is_stale(now, now - Duration::seconds(180), 180));
    // One second under threshold -> not stale.
    assert!(!lease_is_stale(now, now - Duration::seconds(179), 180));
    // Well above threshold -> stale.
    assert!(lease_is_stale(now, now - Duration::seconds(3600), 180));
    // Very recent -> not stale.
    assert!(!lease_is_stale(now, now - Duration::seconds(10), 180));
}

#[test]
fn lease_is_stale_clamps_clock_skew_to_zero() {
    let now = Utc::now();
    // Acquired slightly "in the future" (clock skew): treated as zero
    // elapsed, never stale.
    assert!(!lease_is_stale(now, now + Duration::seconds(5), 1));
}

#[test]
fn classify_takeover_same_owner_returns_idempotent_renew() {
    let locked = lock_row_with(Some("conversation:1"), 30);
    match classify_takeover(&locked, "conversation:1", "conversation:2", 180, false) {
        TakeoverDecision::Update { took_over_foreign } => assert!(!took_over_foreign),
        other => panic!("expected idempotent renew, got {other:?}"),
    }
    // Force does not change the idempotent same-owner path.
    match classify_takeover(&locked, "conversation:1", "conversation:2", 180, true) {
        TakeoverDecision::Update { took_over_foreign } => assert!(!took_over_foreign),
        other => panic!("expected idempotent renew with force, got {other:?}"),
    }
}

#[test]
fn classify_takeover_foreign_owner_above_threshold_returns_foreign_update() {
    let locked = lock_row_with(Some("conversation:1"), 3600);
    match classify_takeover(&locked, "conversation:2", "conversation:1", 180, false) {
        TakeoverDecision::Update { took_over_foreign } => assert!(took_over_foreign),
        other => panic!("expected foreign update, got {other:?}"),
    }
}

#[test]
fn classify_takeover_foreign_owner_inside_window_returns_live_lease() {
    let locked = lock_row_with(Some("conversation:1"), 30);
    match classify_takeover(&locked, "conversation:2", "conversation:1", 180, false) {
        TakeoverDecision::Conflict(TakeoverConflictReason::LiveLease) => {}
        other => panic!("expected live_lease conflict, got {other:?}"),
    }
}

#[test]
fn classify_force_takeover_live_lease_returns_foreign_update() {
    // Explicit user-confirmed force bypasses the stale window but still
    // requires the expected-owner CAS.
    let locked = lock_row_with(Some("conversation:1"), 30);
    match classify_takeover(&locked, "conversation:2", "conversation:1", 180, true) {
        TakeoverDecision::Update { took_over_foreign } => assert!(took_over_foreign),
        other => panic!("expected force foreign update, got {other:?}"),
    }
}

#[test]
fn classify_force_takeover_owner_race_returns_mismatch() {
    // Owner changed between the caller's status read and the takeover:
    // force must not bypass the expected-owner CAS.
    let locked = lock_row_with(Some("conversation:9"), 30);
    match classify_takeover(&locked, "conversation:2", "conversation:1", 180, true) {
        TakeoverDecision::Conflict(TakeoverConflictReason::ExpectedOwnerMismatch) => {}
        other => panic!("expected expected_owner_mismatch with force, got {other:?}"),
    }
}

#[test]
fn classify_force_takeover_without_lease_returns_no_lease() {
    // Force does not create a lease from nothing; callers must use the
    // ordinary acquire endpoint when no lease is held.
    let locked = lock_row_with(None, 0);
    match classify_takeover(&locked, "conversation:2", "conversation:1", 180, true) {
        TakeoverDecision::Conflict(TakeoverConflictReason::NoLease) => {}
        other => panic!("expected no_lease conflict with force, got {other:?}"),
    }
}

#[test]
fn classify_takeover_no_lease_returns_no_lease_conflict() {
    let locked = lock_row_with(None, 0);
    match classify_takeover(&locked, "conversation:2", "conversation:1", 180, false) {
        TakeoverDecision::Conflict(TakeoverConflictReason::NoLease) => {}
        other => panic!("expected no_lease conflict, got {other:?}"),
    }
}

#[test]
fn classify_takeover_unexpected_owner_returns_expected_owner_mismatch() {
    let locked = lock_row_with(Some("conversation:9"), 3600);
    match classify_takeover(&locked, "conversation:2", "conversation:1", 180, false) {
        TakeoverDecision::Conflict(TakeoverConflictReason::ExpectedOwnerMismatch) => {}
        other => panic!("expected expected_owner_mismatch conflict, got {other:?}"),
    }
}

#[test]
fn takeover_commit_gate_blocks_foreign_handover_on_quiescence_failure() {
    // A foreign takeover with failed cancellation must not commit:
    // rolling back leaves the previous owner in place so the identical
    // request is safe to retry and no caller path resumes the new run
    // alongside old execution.
    assert!(!takeover_commit_allowed(true, false));
    assert!(takeover_commit_allowed(true, true));
    // Same-owner renews displace nobody; the cancellation outcome
    // (always default/unattempted there) must not block them.
    assert!(takeover_commit_allowed(false, true));
    assert!(takeover_commit_allowed(false, false));
}

#[test]
fn takeover_conflict_reason_strings_are_stable() {
    // Wire-format identifiers are part of the public OpenAPI contract;
    // any change here is a breaking API change for stock callers.
    assert_eq!(TakeoverConflictReason::NoLease.as_str(), "no_lease");
    assert_eq!(TakeoverConflictReason::LiveLease.as_str(), "live_lease");
    assert_eq!(
        TakeoverConflictReason::ExpectedOwnerMismatch.as_str(),
        "expected_owner_mismatch"
    );
    assert_eq!(TakeoverConflictReason::NotActive.as_str(), "not_active");
}

#[test]
fn lease_status_response_serializes_stale_for_a_live_owner() {
    let db_now = Utc::now();
    let row = WorkspaceLeaseStatusRow {
        workspace_binding_id: 1,
        application_id: 1,
        workspace_key: "code_project:abc".to_string(),
        is_active: true,
        lifecycle_state: "active".to_string(),
        resource_kind: Some("code_project".to_string()),
        resource_id: Some("abc".to_string()),
        write_lease_owner: Some("conversation:1".to_string()),
        write_lease_acquired_at: Some(db_now - Duration::seconds(180)),
        write_lease_expires_at: Some(db_now + Duration::seconds(600)),
        db_now,
    };

    let value = serde_json::to_value(workspace_lease_status_response(row, 180)).expect("serialize");
    assert_eq!(value["stale"], true);
    assert_eq!(value["stale_threshold_seconds"], 180);
}

#[tokio::test]
async fn takeover_cancellation_passthrough_on_fast_success() {
    let outcome = await_takeover_cancellation_with_timeout(
        async {
            crate::db::types::WorkspaceLeaseCancellationOutcome {
                attempted: true,
                succeeded: true,
                sessions_cancelled: 2,
                error: None,
            }
        },
        std::time::Duration::from_millis(500),
    )
    .await;
    assert!(outcome.succeeded);
    assert_eq!(outcome.sessions_cancelled, 2);
}

#[tokio::test]
async fn takeover_cancellation_timeout_fails_closed_without_lease_change() {
    // A hanging runner (e.g. disconnected manager waiting the full
    // 3660s broker timeout) must not pin the row lock: the bounded
    // wait fails closed so the caller rolls back and retries.
    let outcome = await_takeover_cancellation_with_timeout(
        async {
            std::future::pending::<crate::db::types::WorkspaceLeaseCancellationOutcome>().await
        },
        std::time::Duration::from_millis(50),
    )
    .await;
    assert!(outcome.attempted);
    assert!(!outcome.succeeded);
    assert!(
        outcome
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("timed out")
    );
    // The commit gate must block on this outcome.
    assert!(!takeover_commit_allowed(true, outcome.succeeded));
}

#[test]
fn takeover_cancellation_timeout_is_bounded_well_below_runner_job_timeout() {
    // Guard against regressing the row-lock pin back toward the ~1h
    // runner job timeout: the takeover bound must stay on the order
    // of seconds.
    assert!(super::workspace_takeover::TAKEOVER_CANCELLATION_TIMEOUT.as_secs() <= 60);
    assert!(super::workspace_takeover::TAKEOVER_CANCELLATION_TIMEOUT.as_secs() >= 5);
}

#[test]
fn lease_status_response_serializes_false_without_a_lease() {
    let db_now = Utc::now();
    let row = WorkspaceLeaseStatusRow {
        workspace_binding_id: 1,
        application_id: 1,
        workspace_key: "code_project:abc".to_string(),
        is_active: true,
        lifecycle_state: "active".to_string(),
        resource_kind: Some("code_project".to_string()),
        resource_id: Some("abc".to_string()),
        write_lease_owner: None,
        write_lease_acquired_at: None,
        write_lease_expires_at: None,
        db_now,
    };

    let value = serde_json::to_value(workspace_lease_status_response(row, 180)).expect("serialize");
    assert_eq!(value["stale"], false);
}
