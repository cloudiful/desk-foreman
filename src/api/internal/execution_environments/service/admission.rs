//! Owner-scoped admission for the private-service gateway (issue 749 P2b).
//!
//! Admission reads exactly one owner-scoped row and turns it into a decision.
//! The row selects no user, no volume and no credential: the only facts the
//! gateway is allowed to know are the fences it must check and the manager it
//! must reach.
//!
//! The decision is deliberately pure so it can be tested without a database,
//! and the database read around it stays thin enough to read in one go.

use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    db::{
        execution_environments::{
            ServiceEnvironment, find_execution_environment, find_execution_environment_for_service,
        },
        queries::find_workspace_binding_by_id,
        types::WorkspaceBindingResponse,
        workspace_bindings::scope_admission::validate_scope_binding,
    },
    error::AppError,
};

/// What admission decided, for a caller that has been let through.
#[derive(Clone, Debug)]
pub struct ServiceAdmission {
    /// The manager this environment is immutably bound to. It is never
    /// taken from the request.
    pub manager_id: i64,
}

/// Why one environment may not serve right now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FenceOutcome {
    /// Ready, bound, and both fences agree with the caller's header.
    Servable,
    /// The fences agree and a manager is bound, but no live tunnel could be
    /// reached. A relay outage is temporary, so it is reported as
    /// unavailability rather than as a caller conflict.
    ManagerUnreachable,
    /// The environment is not ready, not provisioned, or the presented
    /// generation does not match both stored fences.
    NotServable,
}

/// Decide from one owner-scoped row whether this request may be served.
///
/// `presented_generation` is the caller's expected fence, not an identity: it
/// must equal both the authorized generation and the provisioned candidate.
pub fn fence_outcome(row: &ServiceEnvironment, presented_generation: i64) -> FenceOutcome {
    if row.is_servable(presented_generation) {
        FenceOutcome::Servable
    } else if row.is_bound_but_not_servable(presented_generation) {
        FenceOutcome::ManagerUnreachable
    } else {
        FenceOutcome::NotServable
    }
}

/// Map a refusal to its HTTP outcome.
///
/// A foreign and an unknown environment are both absent, so this cannot be
/// used to learn whether an identifier exists.
pub fn refuse_admission(outcome: FenceOutcome) -> AppError {
    match outcome {
        FenceOutcome::Servable => {
            AppError::internal(anyhow::anyhow!("unreachable admission state"))
        }
        FenceOutcome::ManagerUnreachable => {
            AppError::service_unavailable("the environment's manager relay is unavailable")
        }
        FenceOutcome::NotServable => {
            AppError::conflict("execution environment is not serving at the presented generation")
        }
    }
}

/// Authenticated workspace scope carried from the handler onto the relay.
///
/// Deliberately not `Debug`: the lease owner is caller proof and must never
/// reach a log, a formatted error, or the tunnel.
pub(super) struct ScopeRelayAuth {
    pub(super) application_id: i64,
    pub(super) workspace_binding_id: i64,
    pub(super) lease_owner: String,
}

/// Pure comparison of an environment's stored scope with the authenticated
/// scope and the fresh binding row.
///
/// An unscoped environment must carry no scope auth. A scoped environment
/// requires an auth for the same application and the stored binding, plus a
/// fresh binding whose explicit lease the shared validator accepts.
pub(super) fn validate_service_scope_request(
    application_id: i64,
    stored_workspace_binding_id: Option<i64>,
    auth: Option<&ScopeRelayAuth>,
    fresh_binding: Option<&WorkspaceBindingResponse>,
) -> Result<(), AppError> {
    let Some(stored_binding_id) = stored_workspace_binding_id else {
        return if auth.is_none() {
            Ok(())
        } else {
            Err(scope_denied())
        };
    };
    let Some(auth) = auth else {
        return Err(scope_denied());
    };
    if auth.application_id != application_id || auth.workspace_binding_id != stored_binding_id {
        return Err(scope_denied());
    }
    let Some(fresh_binding) = fresh_binding else {
        return Err(scope_denied());
    };
    validate_scope_binding(
        fresh_binding,
        application_id,
        stored_binding_id,
        Some(auth.lease_owner.as_str()),
    )
    .map_err(|_| scope_denied())
}

/// Re-read the stored scope and its fresh binding, then validate them.
///
/// The environment read is owner-scoped and the binding is read by the stored
/// id, never by a header-selected id, so a request cannot change the target.
pub(super) async fn validate_service_scope(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    auth: Option<&ScopeRelayAuth>,
) -> Result<(), AppError> {
    let stored = find_execution_environment(pool, application_id, environment_id).await?;
    let Some(stored) = stored else {
        return Err(AppError::not_found("execution environment not found"));
    };
    let fresh_binding = match stored.workspace_binding_id {
        Some(binding_id) => find_workspace_binding_by_id(pool, binding_id).await?,
        None => None,
    };
    validate_service_scope_request(
        application_id,
        stored.workspace_binding_id,
        auth,
        fresh_binding.as_ref(),
    )
}

fn scope_denied() -> AppError {
    AppError::forbidden("workspace scope is not authorized for this execution environment")
}

/// Read the owner-scoped row and admit or refuse.
///
/// `presented_generation` is the caller's header, never an identity.
pub(super) async fn admit_service_request(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    presented_generation: i64,
    scope_auth: Option<&ScopeRelayAuth>,
) -> Result<ServiceAdmission, AppError> {
    let row = find_execution_environment_for_service(pool, application_id, environment_id).await?;
    let Some(row) = row else {
        return Err(AppError::not_found("execution environment not found"));
    };
    match fence_outcome(&row, presented_generation) {
        FenceOutcome::Servable => {
            validate_service_scope(pool, application_id, environment_id, scope_auth).await?;
            Ok(ServiceAdmission {
                manager_id: row
                    .assigned_manager_id
                    .expect("a servable environment is bound"),
            })
        }
        outcome => Err(refuse_admission(outcome)),
    }
}

#[cfg(test)]
mod tests {
    use crate::{db::execution_environments::ServiceEnvironment, error::AppError};

    use super::{FenceOutcome, fence_outcome, refuse_admission};

    fn env(
        generation: i64,
        container: Option<i64>,
        manager: Option<i64>,
        state: &str,
    ) -> ServiceEnvironment {
        ServiceEnvironment {
            generation,
            container_generation: container,
            assigned_manager_id: manager,
            state: state.to_string(),
        }
    }

    #[test]
    fn only_a_ready_bound_environment_at_the_presented_generation_serves() {
        assert_eq!(
            fence_outcome(&env(4, Some(4), Some(7), "ready"), 4),
            FenceOutcome::Servable
        );
    }

    #[test]
    fn a_matching_fence_with_a_bound_manager_is_unreachable_not_a_conflict() {
        assert_eq!(
            fence_outcome(&env(4, Some(4), Some(7), "starting"), 4),
            FenceOutcome::ManagerUnreachable
        );
    }

    #[test]
    fn a_wrong_or_absent_fence_is_never_reported_as_a_relay_outage() {
        for row in [
            env(4, Some(4), Some(7), "ready"),
            env(4, None, Some(7), "ready"),
            env(4, Some(4), None, "ready"),
            env(4, Some(5), Some(7), "ready"),
        ] {
            assert_eq!(
                fence_outcome(&row, 3),
                FenceOutcome::NotServable,
                "{row:?} at a stale fence is a caller conflict"
            );
        }
    }

    #[test]
    fn refusals_carry_no_environment_or_manager_detail() {
        assert!(matches!(
            refuse_admission(FenceOutcome::NotServable),
            AppError::Conflict(_)
        ));
        assert!(matches!(
            refuse_admission(FenceOutcome::ManagerUnreachable),
            AppError::ServiceUnavailable(_)
        ));
    }
}

#[cfg(test)]
mod service_scope_tests {
    use chrono::{Duration, Utc};

    use super::{ScopeRelayAuth, validate_service_scope_request};
    use crate::{db::types::WorkspaceBindingResponse, error::AppError};

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

    fn auth() -> ScopeRelayAuth {
        ScopeRelayAuth {
            application_id: APPLICATION_ID,
            workspace_binding_id: BINDING_ID,
            lease_owner: LEASE_OWNER.to_string(),
        }
    }

    fn assert_denied(result: Result<(), AppError>) {
        let error = result.expect_err("a scope mismatch must be refused");
        assert!(
            matches!(error, AppError::Forbidden(_)),
            "unexpected: {error:?}"
        );
        assert!(
            !format!("{error:?}").contains(LEASE_OWNER),
            "a refusal must not carry the lease owner"
        );
    }

    #[test]
    fn service_scope_unscoped_requires_no_scope_auth() {
        assert!(
            validate_service_scope_request(APPLICATION_ID, None, None, None).is_ok(),
            "an unscoped environment keeps its ordinary behaviour"
        );
        let scope = auth();
        assert_denied(validate_service_scope_request(
            APPLICATION_ID,
            None,
            Some(&scope),
            None,
        ));
    }

    #[test]
    fn service_scope_matching_scope_and_live_lease_is_accepted() {
        let scope = auth();
        let fresh = binding();
        assert!(
            validate_service_scope_request(
                APPLICATION_ID,
                Some(BINDING_ID),
                Some(&scope),
                Some(&fresh)
            )
            .is_ok()
        );
    }

    #[test]
    fn service_scope_rejects_missing_or_mismatched_auth() {
        let fresh = binding();
        assert_denied(validate_service_scope_request(
            APPLICATION_ID,
            Some(BINDING_ID),
            None,
            Some(&fresh),
        ));

        let mut wrong_application = auth();
        wrong_application.application_id += 1;
        assert_denied(validate_service_scope_request(
            APPLICATION_ID,
            Some(BINDING_ID),
            Some(&wrong_application),
            Some(&fresh),
        ));

        let mut wrong_binding = auth();
        wrong_binding.workspace_binding_id += 1;
        assert_denied(validate_service_scope_request(
            APPLICATION_ID,
            Some(BINDING_ID),
            Some(&wrong_binding),
            Some(&fresh),
        ));
    }

    #[test]
    fn service_scope_rejects_missing_archived_inactive_and_expired_bindings() {
        let scope = auth();
        assert_denied(validate_service_scope_request(
            APPLICATION_ID,
            Some(BINDING_ID),
            Some(&scope),
            None,
        ));

        let mut archived = binding();
        archived.archived_at = Some(Utc::now());
        assert_denied(validate_service_scope_request(
            APPLICATION_ID,
            Some(BINDING_ID),
            Some(&scope),
            Some(&archived),
        ));

        let mut inactive = binding();
        inactive.is_active = false;
        assert_denied(validate_service_scope_request(
            APPLICATION_ID,
            Some(BINDING_ID),
            Some(&scope),
            Some(&inactive),
        ));

        let mut expired = binding();
        expired.write_lease_expires_at = Some(Utc::now() - Duration::minutes(1));
        assert_denied(validate_service_scope_request(
            APPLICATION_ID,
            Some(BINDING_ID),
            Some(&scope),
            Some(&expired),
        ));

        let mut foreign_lease = binding();
        foreign_lease.write_lease_owner = Some("lease:other".to_string());
        assert_denied(validate_service_scope_request(
            APPLICATION_ID,
            Some(BINDING_ID),
            Some(&scope),
            Some(&foreign_lease),
        ));
    }

    #[test]
    fn service_scope_rejects_a_lease_changed_between_two_validations() {
        let scope = auth();
        let first = binding();
        assert!(
            validate_service_scope_request(
                APPLICATION_ID,
                Some(BINDING_ID),
                Some(&scope),
                Some(&first)
            )
            .is_ok()
        );

        let mut changed = binding();
        changed.write_lease_owner = Some("lease:other".to_string());
        assert_denied(validate_service_scope_request(
            APPLICATION_ID,
            Some(BINDING_ID),
            Some(&scope),
            Some(&changed),
        ));
    }
}
