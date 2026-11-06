//! Pure workspace-scope authority checks (issue 1180 P6).
//!
//! Two consumers will share these: the scoped provision/admission path must
//! validate a locked, fresh binding row against the authenticated identity and
//! its explicit lease, and session observation must recognise only owners that
//! are bound to that workspace. Both functions are pure so they can be reused
//! without a database and tested without a harness.

use crate::db::types::WorkspaceBindingResponse;

/// Validate a fresh, locked binding row against the authenticated scope.
///
/// Checks run in a fixed order so a caller cannot learn a later fact from an
/// earlier refusal: application ownership, authenticated binding identity,
/// resource ownership, active lifecycle/archive state, explicit lease, then the
/// shared write-lease comparison. The lease is passed through untrimmed so a
/// padded credential can never be normalised into a different owner.
pub(crate) fn validate_scope_binding(
    binding: &WorkspaceBindingResponse,
    application_id: i64,
    auth_binding_id: i64,
    lease_owner: Option<&str>,
) -> Result<(), String> {
    if binding.application_id != application_id {
        return Err("workspace scope is not authorized for this application".to_string());
    }
    if binding.workspace_binding_id != auth_binding_id {
        return Err("workspace scope does not match the authenticated binding".to_string());
    }
    let resource_owned = binding
        .resource_kind
        .as_deref()
        .is_some_and(|kind| !kind.trim().is_empty())
        && binding
            .resource_id
            .as_deref()
            .is_some_and(|id| !id.trim().is_empty());
    if !resource_owned {
        return Err("workspace scope requires a resource-owned binding".to_string());
    }
    if !binding.is_active || binding.lifecycle_state != "active" || binding.archived_at.is_some() {
        return Err("workspace scope is not active".to_string());
    }
    let Some(lease_owner) = lease_owner.filter(|owner| !owner.trim().is_empty()) else {
        return Err("workspace scope requires an explicit write lease".to_string());
    };
    crate::actor::check_write_lease(binding, Some(lease_owner))
}

/// True when an execution owner is bound to the expected workspace binding.
///
/// Only the two binding-carrying owner variants match; personal and
/// environment owners never do. A nonpositive binding id never matches.
pub(crate) fn session_owner_matches_scope(
    owner: &runner_protocol::RunnerOwner,
    workspace_binding_id: i64,
) -> bool {
    workspace_binding_id > 0 && owner.workspace_binding_id() == Some(workspace_binding_id)
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use runner_protocol::RunnerOwner;

    use super::{session_owner_matches_scope, validate_scope_binding};
    use crate::db::types::WorkspaceBindingResponse;

    const APPLICATION_ID: i64 = 7;
    const BINDING_ID: i64 = 11;
    const LEASE_OWNER: &str = "lease:fixture";

    fn binding() -> WorkspaceBindingResponse {
        let now = Utc::now();
        WorkspaceBindingResponse {
            workspace_binding_id: BINDING_ID,
            application_id: APPLICATION_ID,
            external_user_id: "fixture-user".to_string(),
            workspace_key: "artifact:fixture".to_string(),
            external_user_hash: "fixture-hash".to_string(),
            workspace_root: "/workspace/fixture".to_string(),
            is_active: true,
            last_used_at: now,
            created_at: now,
            updated_at: now,
            lifecycle_state: "active".to_string(),
            archived_at: None,
            resource_kind: Some("artifact".to_string()),
            resource_id: Some("fixture".to_string()),
            write_lease_owner: Some(LEASE_OWNER.to_string()),
            write_lease_acquired_at: Some(now),
            write_lease_expires_at: Some(now + Duration::hours(1)),
        }
    }

    enum Expected {
        Ok,
        Message(&'static str),
        LeaseDenied,
    }

    fn check(binding: &WorkspaceBindingResponse, lease: Option<&str>, expected: Expected) {
        let result = validate_scope_binding(
            binding,
            APPLICATION_ID,
            BINDING_ID,
            lease.or(Some(LEASE_OWNER)),
        );
        match expected {
            Expected::Ok => assert!(result.is_ok(), "unexpected error: {result:?}"),
            Expected::Message(message) => {
                assert_eq!(result.err().as_deref(), Some(message), "unexpected result");
            }
            Expected::LeaseDenied => {
                let error = result.expect_err("a stale lease must be refused");
                assert!(
                    error.contains("no write lease held by this session"),
                    "unexpected error: {error}"
                );
            }
        }
    }

    #[test]
    fn validate_scope_binding_table() {
        check(&binding(), None, Expected::Ok);

        let mut wrong_application = binding();
        wrong_application.application_id += 1;
        check(
            &wrong_application,
            None,
            Expected::Message("workspace scope is not authorized for this application"),
        );

        let mut wrong_binding = binding();
        wrong_binding.workspace_binding_id += 1;
        check(
            &wrong_binding,
            None,
            Expected::Message("workspace scope does not match the authenticated binding"),
        );

        let mut missing_kind = binding();
        missing_kind.resource_kind = None;
        check(
            &missing_kind,
            None,
            Expected::Message("workspace scope requires a resource-owned binding"),
        );

        let mut blank_resource = binding();
        blank_resource.resource_id = Some("   ".to_string());
        check(
            &blank_resource,
            None,
            Expected::Message("workspace scope requires a resource-owned binding"),
        );

        let mut inactive = binding();
        inactive.is_active = false;
        check(
            &inactive,
            None,
            Expected::Message("workspace scope is not active"),
        );

        let mut archived = binding();
        archived.archived_at = Some(Utc::now());
        check(
            &archived,
            None,
            Expected::Message("workspace scope is not active"),
        );

        let mut not_live = binding();
        not_live.lifecycle_state = "archived".to_string();
        check(
            &not_live,
            None,
            Expected::Message("workspace scope is not active"),
        );

        let mut no_lease = binding();
        no_lease.write_lease_owner = None;
        check(
            &no_lease,
            Some(""),
            Expected::Message("workspace scope requires an explicit write lease"),
        );

        let mut foreign_lease = binding();
        foreign_lease.write_lease_owner = Some("lease:other".to_string());
        check(&foreign_lease, None, Expected::LeaseDenied);
    }

    #[test]
    fn session_owner_matches_scope_variants() {
        let bound = RunnerOwner::WorkspaceBinding {
            workspace_binding_id: BINDING_ID,
        };
        let principal = RunnerOwner::ApplicationPrincipal {
            application_id: APPLICATION_ID,
            workspace_binding_id: BINDING_ID,
            principal_digest: "0".repeat(64),
        };
        let internal_user = RunnerOwner::InternalUser { user_id: 3 };
        let environment = RunnerOwner::ExecutionEnvironment {
            environment_id: "fixture-environment".to_string(),
            generation: 1,
        };

        assert!(session_owner_matches_scope(&bound, BINDING_ID));
        assert!(session_owner_matches_scope(&principal, BINDING_ID));
        assert!(!session_owner_matches_scope(&bound, BINDING_ID + 1));
        assert!(!session_owner_matches_scope(&internal_user, BINDING_ID));
        assert!(!session_owner_matches_scope(&environment, BINDING_ID));
        assert!(!session_owner_matches_scope(&bound, 0));
    }
}
