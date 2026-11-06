//! Pure write-lease admission helpers.
//!
//! Extracted from `actor.rs` to keep each file under the 400-line cap.
//! Re-exported via `crate::actor::{check_write_lease, admit_fenced_write}`
//! so existing callers (`application.rs`, `tools/shared.rs`) keep working.

use chrono::Utc;

use crate::db::types::WorkspaceBindingResponse;

/// Pure write-lease admission check shared by the snapshot helper above and
/// the fenced database re-validation in the tool layer.
///
/// `binding` must be the freshest available row (fresh database read on the
/// tool path, authentication snapshot only for non-fenced callers).
/// Resource-owned workspaces require `lease_owner` to equal the binding's
/// current `write_lease_owner` with a future `write_lease_expires_at`.
/// Per-user workspaces (no `resource_kind`) are unaffected.
pub fn check_write_lease(
    binding: &WorkspaceBindingResponse,
    lease_owner: Option<&str>,
) -> Result<(), String> {
    if binding.resource_kind.is_none() {
        return Ok(());
    }
    if binding.lifecycle_state != "active" || !binding.is_active {
        return Err(write_lease_denied_message());
    }
    let lease_owner = lease_owner.unwrap_or_default();
    let holds_lease = binding.write_lease_owner.as_deref() == Some(lease_owner)
        && !lease_owner.is_empty()
        && binding
            .write_lease_expires_at
            .is_some_and(|expires| expires > Utc::now());
    if holds_lease {
        return Ok(());
    }
    Err(write_lease_denied_message())
}

fn write_lease_denied_message() -> String {
    "workspace is read-only: no write lease held by this session. Acquire the write lease (or take it over) before running mutating commands"
        .to_string()
}

fn personal_write_denied_message() -> String {
    "workspace is not available for writes".to_string()
}

/// Pure personal-workspace admission over the binding row locked by
/// [`crate::db::workspace_bindings::lock_workspace_binding_for_write`].
///
/// A bound personal workspace needs no write lease: it only has to still be
/// the same active, non-resource binding that authentication saw. The locked
/// fresh row must match the snapshot's binding and application identity and
/// remain active, `active`-lifecycle, and unarchived. Missing, resetting,
/// archived, resource-owned, or identity-mismatched rows are refused.
pub fn admit_personal_write(
    fresh: Option<&WorkspaceBindingResponse>,
    snapshot: &WorkspaceBindingResponse,
) -> Result<(), String> {
    let Some(fresh) = fresh else {
        return Err(personal_write_denied_message());
    };
    if fresh.workspace_binding_id != snapshot.workspace_binding_id
        || fresh.application_id != snapshot.application_id
        || fresh.resource_kind.is_some()
        || !fresh.is_active
        || fresh.lifecycle_state != "active"
        || fresh.archived_at.is_some()
    {
        return Err(personal_write_denied_message());
    }
    Ok(())
}

/// Pure fenced admission over the binding row locked by
/// [`crate::db::workspace_bindings::lock_workspace_binding_for_write`].
///
/// The row must be the locked fresh read held in the caller's open
/// transaction, never the authentication-time snapshot: when a takeover
/// commits between authentication and the write, the locked fresh row
/// shows the new owner and the old owner's write is denied instead of
/// crossing the handover. A vanished binding denies the same way as a
/// lease mismatch so callers cannot distinguish the cases.
pub fn admit_fenced_write(
    fresh: Option<&WorkspaceBindingResponse>,
    lease_owner: Option<&str>,
) -> Result<(), String> {
    let Some(fresh) = fresh else {
        return Err(write_lease_denied_message());
    };
    check_write_lease(fresh, lease_owner)
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use super::{admit_fenced_write, admit_personal_write, check_write_lease};
    use crate::db::types::WorkspaceBindingResponse;

    fn personal_binding(
        workspace_binding_id: i64,
        application_id: i64,
        resource_kind: Option<&str>,
    ) -> WorkspaceBindingResponse {
        let now = Utc::now();
        WorkspaceBindingResponse {
            workspace_binding_id,
            application_id,
            external_user_id: "user".to_string(),
            workspace_key: "personal".to_string(),
            external_user_hash: "hash".to_string(),
            workspace_root: "/tmp/ws".to_string(),
            is_active: true,
            last_used_at: now,
            created_at: now,
            updated_at: now,
            lifecycle_state: "active".to_string(),
            archived_at: None,
            resource_kind: resource_kind.map(str::to_string),
            resource_id: resource_kind.map(|_| "resource".to_string()),
            write_lease_owner: None,
            write_lease_acquired_at: None,
            write_lease_expires_at: None,
        }
    }

    #[test]
    fn fenced_check_rejects_old_owner_after_handover() {
        let now = Utc::now();
        let handover_binding = WorkspaceBindingResponse {
            workspace_binding_id: 1,
            application_id: 1,
            external_user_id: "__resource__".to_string(),
            workspace_key: "code_project:abc".to_string(),
            external_user_hash: "hash".to_string(),
            workspace_root: "/tmp/ws".to_string(),
            is_active: true,
            last_used_at: now,
            created_at: now,
            updated_at: now,
            lifecycle_state: "active".to_string(),
            archived_at: None,
            resource_kind: Some("code_project".to_string()),
            resource_id: Some("abc".to_string()),
            write_lease_owner: Some("conversation:2".to_string()),
            write_lease_acquired_at: Some(now),
            write_lease_expires_at: Some(now + Duration::minutes(10)),
        };
        // The displaced owner was admitted at request time but must not cross
        // the handover once the fresh row shows the new owner.
        assert!(check_write_lease(&handover_binding, Some("conversation:1")).is_err());
        assert!(check_write_lease(&handover_binding, Some("conversation:2")).is_ok());
        assert!(check_write_lease(&handover_binding, None).is_err());
        // The same contract through the locked-row admission entry point
        // used by every fenced mutating operation.
        assert!(admit_fenced_write(Some(&handover_binding), Some("conversation:1")).is_err());
        assert!(admit_fenced_write(Some(&handover_binding), Some("conversation:2")).is_ok());
        // A binding that vanished between authentication and the locked
        // read denies like a lease mismatch.
        assert!(admit_fenced_write(None, Some("conversation:2")).is_err());
    }

    #[test]
    fn fenced_check_rejects_inactive_binding_even_with_matching_owner() {
        use super::check_write_lease;

        let now = Utc::now();
        let archived = WorkspaceBindingResponse {
            workspace_binding_id: 1,
            application_id: 1,
            external_user_id: "__resource__".to_string(),
            workspace_key: "code_project:abc".to_string(),
            external_user_hash: "hash".to_string(),
            workspace_root: "/tmp/ws".to_string(),
            is_active: false,
            last_used_at: now,
            created_at: now,
            updated_at: now,
            lifecycle_state: "archived".to_string(),
            archived_at: Some(now),
            resource_kind: Some("code_project".to_string()),
            resource_id: Some("abc".to_string()),
            write_lease_owner: Some("conversation:1".to_string()),
            write_lease_acquired_at: Some(now),
            write_lease_expires_at: Some(now + Duration::minutes(10)),
        };
        assert!(check_write_lease(&archived, Some("conversation:1")).is_err());
    }

    #[test]
    fn personal_write_requires_active_locked_binding() {
        let snapshot = personal_binding(1, 1, None);
        assert!(admit_personal_write(Some(&snapshot), &snapshot).is_ok());
        assert!(
            admit_personal_write(None, &snapshot).is_err(),
            "a vanished locked row must fail closed"
        );
    }

    #[test]
    fn personal_write_rejects_inactive_archived_resetting_and_identity_mismatch() {
        let snapshot = personal_binding(1, 1, None);
        let mut inactive = snapshot.clone();
        inactive.is_active = false;
        let mut archived = snapshot.clone();
        archived.lifecycle_state = "archived".to_string();
        let mut resetting = snapshot.clone();
        resetting.lifecycle_state = "resetting".to_string();
        let mut archived_at = snapshot.clone();
        archived_at.archived_at = Some(Utc::now());
        let wrong_application = personal_binding(1, 2, None);
        let wrong_binding = personal_binding(2, 1, None);
        let resource = personal_binding(1, 1, Some("code_project"));
        for fresh in [
            inactive,
            archived,
            resetting,
            archived_at,
            wrong_application,
            wrong_binding,
            resource,
        ] {
            assert!(
                admit_personal_write(Some(&fresh), &snapshot).is_err(),
                "an inactive, archived, resetting, resource, or mismatched row must be refused"
            );
        }
    }

    #[test]
    fn personal_write_ignores_lease() {
        let snapshot = personal_binding(1, 1, None);
        assert!(admit_personal_write(Some(&snapshot), &snapshot).is_ok());
        let mut foreign_lease = snapshot.clone();
        foreign_lease.write_lease_owner = Some("conversation:other".to_string());
        foreign_lease.write_lease_acquired_at = Some(Utc::now());
        foreign_lease.write_lease_expires_at = Some(Utc::now() - Duration::minutes(1));
        assert!(
            admit_personal_write(Some(&foreign_lease), &snapshot).is_ok(),
            "personal admission never demands or rejects a lease"
        );
        let mut no_lease = snapshot.clone();
        no_lease.write_lease_owner = None;
        no_lease.write_lease_expires_at = None;
        assert!(admit_personal_write(Some(&no_lease), &snapshot).is_ok());
    }
}
