use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

use crate::db::types::{
    ListWorkspaceBindingsParams, Page, TakeoverConflictReason, WorkspaceBindingResponse,
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
    },
    /// The takeover was rejected with a machine-readable reason. The
    /// binding row is included so callers can drive a retry strategy.
    Conflict {
        reason: TakeoverConflictReason,
        current: WorkspaceLeaseStatusRow,
    },
    /// The binding does not exist.
    NotFound,
}

pub async fn find_workspace_binding(
    pool: &PgPool,
    application_id: i64,
    external_user_id: &str,
    workspace_key: &str,
) -> anyhow::Result<Option<WorkspaceBindingResponse>> {
    sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!("../sql/find_workspace_binding.sql"))
        .bind(application_id)
        .bind(external_user_id)
        .bind(workspace_key)
        .fetch_optional(pool)
        .await
        .map_err(Into::into)
}

pub async fn find_workspace_binding_any(
    pool: &PgPool,
    application_id: i64,
    external_user_id: &str,
    workspace_key: &str,
) -> anyhow::Result<Option<WorkspaceBindingResponse>> {
    sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!(
        "../sql/find_workspace_binding_any.sql"
    ))
    .bind(application_id)
    .bind(external_user_id)
    .bind(workspace_key)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn create_workspace_binding(
    pool: &PgPool,
    application_id: i64,
    external_user_id: &str,
    workspace_key: &str,
    workspace_root: &str,
    resource_kind: Option<&str>,
    resource_id: Option<&str>,
) -> anyhow::Result<WorkspaceBindingResponse> {
    let external_user_hash = external_user_hash(external_user_id);
    let binding = sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!(
        "../sql/create_workspace_binding.sql"
    ))
    .bind(application_id)
    .bind(external_user_id)
    .bind(workspace_key)
    .bind(external_user_hash)
    .bind(workspace_root)
    .bind(resource_kind)
    .bind(resource_id)
    .fetch_optional(pool)
    .await?;
    if let Some(binding) = binding {
        return Ok(binding);
    }
    // Concurrent first request may have won the insert; return the existing row.
    find_workspace_binding_any(pool, application_id, external_user_id, workspace_key)
        .await?
        .ok_or_else(|| anyhow::anyhow!("workspace binding vanished after create"))
}

pub async fn find_workspace_binding_by_resource(
    pool: &PgPool,
    application_id: i64,
    resource_kind: &str,
    resource_id: &str,
) -> anyhow::Result<Option<WorkspaceBindingResponse>> {
    sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!(
        "../sql/find_workspace_binding_by_resource.sql"
    ))
    .bind(application_id)
    .bind(resource_kind)
    .bind(resource_id)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn acquire_workspace_write_lease(
    pool: &PgPool,
    workspace_binding_id: i64,
    owner: &str,
    ttl_seconds: u64,
) -> anyhow::Result<Option<WorkspaceBindingResponse>> {
    sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!(
        "../sql/acquire_workspace_write_lease.sql"
    ))
    .bind(owner)
    .bind(i64::try_from(ttl_seconds).unwrap_or(i64::MAX))
    .bind(workspace_binding_id)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn release_workspace_write_lease(
    pool: &PgPool,
    workspace_binding_id: i64,
    owner: &str,
) -> anyhow::Result<Option<WorkspaceBindingResponse>> {
    sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!(
        "../sql/release_workspace_write_lease.sql"
    ))
    .bind(workspace_binding_id)
    .bind(owner)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

/// Atomic, stale-guarded write-lease takeover.
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
/// Classification:
///   * `new_owner` already holds the lease      -> idempotent renew,
///     `took_over_foreign = false`, no session cancellation.
///   * `expected_owner` holds the lease and the
///     last refresh is at least `stale_threshold_seconds` old -> foreign
///     takeover, `took_over_foreign = true`. The caller should cancel
///     binding-scoped runner sessions.
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

    let decision = classify_takeover(&locked, new_owner, expected_owner, stale_threshold_seconds);
    match decision {
        TakeoverDecision::Update { took_over_foreign } => {
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
enum TakeoverDecision {
    Update { took_over_foreign: bool },
    Conflict(TakeoverConflictReason),
}

/// Pure classifier for the takeover decision. Kept as a free function so
/// the stale timestamp semantics can be unit-tested without a database.
fn classify_takeover(
    locked: &WorkspaceLeaseTakeoverLockRow,
    new_owner: &str,
    expected_owner: &str,
    stale_threshold_seconds: u64,
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
    } else if lease_owner == Some(expected_owner) && is_stale {
        // Foreign takeover: the lease matches the caller's expected
        // owner and has not been refreshed within the stale window.
        TakeoverDecision::Update {
            took_over_foreign: true,
        }
    } else if lease_owner.is_none() {
        TakeoverDecision::Conflict(TakeoverConflictReason::NoLease)
    } else if lease_owner == Some(expected_owner) {
        // The lease matches expected_owner but is still inside the
        // stale window; caller should wait or retry.
        TakeoverDecision::Conflict(TakeoverConflictReason::LiveLease)
    } else {
        // The lease owner differs from expected_owner (and from
        // new_owner); the writer that refreshed the lease has changed.
        TakeoverDecision::Conflict(TakeoverConflictReason::ExpectedOwnerMismatch)
    }
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

/// Read the current lease state for an active resource workspace binding.
///
/// Scoped to the caller's application id and resource identity so that
/// callers cannot enumerate leases for unrelated bindings.
pub async fn find_active_resource_workspace_lease(
    pool: &PgPool,
    application_id: i64,
    resource_kind: &str,
    resource_id: &str,
) -> anyhow::Result<Option<WorkspaceLeaseStatusRow>> {
    sqlx::query_as::<_, WorkspaceLeaseStatusRow>(include_str!(
        "../sql/find_active_resource_workspace_lease.sql"
    ))
    .bind(application_id)
    .bind(resource_kind)
    .bind(resource_id)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn touch_workspace_binding(
    pool: &PgPool,
    workspace_binding_id: i64,
) -> anyhow::Result<()> {
    sqlx::query(include_str!("../sql/touch_workspace_binding.sql"))
        .bind(workspace_binding_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_workspace_binding_state(
    pool: &PgPool,
    workspace_binding_id: i64,
    state: &str,
) -> anyhow::Result<Option<WorkspaceBindingResponse>> {
    sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!(
        "../sql/set_workspace_binding_state.sql"
    ))
    .bind(workspace_binding_id)
    .bind(state)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn list_archived_workspace_bindings(
    pool: &PgPool,
    archived_before: DateTime<Utc>,
) -> anyhow::Result<Vec<WorkspaceBindingResponse>> {
    sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!(
        "../sql/list_archived_workspace_bindings.sql"
    ))
    .bind(archived_before)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn delete_workspace_binding(
    pool: &PgPool,
    workspace_binding_id: i64,
) -> anyhow::Result<Option<String>> {
    sqlx::query(include_str!("../sql/delete_workspace_binding.sql"))
        .bind(workspace_binding_id)
        .fetch_optional(pool)
        .await
        .map(|row| row.map(|row| row.get("workspace_root")))
        .map_err(Into::into)
}

pub async fn find_workspace_binding_by_id(
    pool: &PgPool,
    workspace_binding_id: i64,
) -> anyhow::Result<Option<WorkspaceBindingResponse>> {
    sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!(
        "../sql/find_workspace_binding_by_id.sql"
    ))
    .bind(workspace_binding_id)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn list_workspace_bindings(
    pool: &PgPool,
    params: &ListWorkspaceBindingsParams,
) -> anyhow::Result<Page<WorkspaceBindingResponse>> {
    let limit = params.limit.unwrap_or(20).clamp(1, 200);
    let offset = params.offset.unwrap_or(0).max(0);
    let total_row = sqlx::query(include_str!("../sql/count_workspace_bindings.sql"))
        .bind(params.application_id)
        .bind(&params.external_user_id)
        .bind(&params.workspace_key)
        .bind(params.is_active)
        .bind(&params.lifecycle_state)
        .fetch_one(pool)
        .await?;
    let total = total_row.get("count");
    let rows = sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!(
        "../sql/list_workspace_bindings.sql"
    ))
    .bind(params.application_id)
    .bind(&params.external_user_id)
    .bind(&params.workspace_key)
    .bind(params.is_active)
    .bind(&params.lifecycle_state)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(Page {
        items: rows,
        total,
        limit,
        offset,
    })
}

pub fn external_user_hash(external_user_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(external_user_id.as_bytes());
    let digest = hasher.finalize();
    digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use super::{
        TakeoverDecision, WorkspaceLeaseTakeoverLockRow, classify_takeover, lease_is_stale,
        workspace_lease_status_response,
    };
    use crate::db::types::{TakeoverConflictReason, WorkspaceLeaseStatusRow};

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
        match classify_takeover(&locked, "conversation:1", "conversation:2", 180) {
            TakeoverDecision::Update { took_over_foreign } => assert!(!took_over_foreign),
            other => panic!("expected idempotent renew, got {other:?}"),
        }
    }

    #[test]
    fn classify_takeover_foreign_owner_above_threshold_returns_foreign_update() {
        let locked = lock_row_with(Some("conversation:1"), 3600);
        match classify_takeover(&locked, "conversation:2", "conversation:1", 180) {
            TakeoverDecision::Update { took_over_foreign } => assert!(took_over_foreign),
            other => panic!("expected foreign update, got {other:?}"),
        }
    }

    #[test]
    fn classify_takeover_foreign_owner_inside_window_returns_live_lease() {
        let locked = lock_row_with(Some("conversation:1"), 30);
        match classify_takeover(&locked, "conversation:2", "conversation:1", 180) {
            TakeoverDecision::Conflict(TakeoverConflictReason::LiveLease) => {}
            other => panic!("expected live_lease conflict, got {other:?}"),
        }
    }

    #[test]
    fn classify_takeover_no_lease_returns_no_lease_conflict() {
        let locked = lock_row_with(None, 0);
        match classify_takeover(&locked, "conversation:2", "conversation:1", 180) {
            TakeoverDecision::Conflict(TakeoverConflictReason::NoLease) => {}
            other => panic!("expected no_lease conflict, got {other:?}"),
        }
    }

    #[test]
    fn classify_takeover_unexpected_owner_returns_expected_owner_mismatch() {
        let locked = lock_row_with(Some("conversation:9"), 3600);
        match classify_takeover(&locked, "conversation:2", "conversation:1", 180) {
            TakeoverDecision::Conflict(TakeoverConflictReason::ExpectedOwnerMismatch) => {}
            other => panic!("expected expected_owner_mismatch conflict, got {other:?}"),
        }
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

        let value =
            serde_json::to_value(workspace_lease_status_response(row, 180)).expect("serialize");
        assert_eq!(value["stale"], true);
        assert_eq!(value["stale_threshold_seconds"], 180);
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

        let value =
            serde_json::to_value(workspace_lease_status_response(row, 180)).expect("serialize");
        assert_eq!(value["stale"], false);
    }
}
