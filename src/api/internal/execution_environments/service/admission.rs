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
    db::execution_environments::{ServiceEnvironment, find_execution_environment_for_service},
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

/// Read the owner-scoped row and admit or refuse.
///
/// `presented_generation` is the caller's header, never an identity.
pub async fn admit_service_request(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    presented_generation: i64,
) -> Result<ServiceAdmission, AppError> {
    let row = find_execution_environment_for_service(pool, application_id, environment_id).await?;
    let Some(row) = row else {
        return Err(AppError::not_found("execution environment not found"));
    };
    match fence_outcome(&row, presented_generation) {
        FenceOutcome::Servable => Ok(ServiceAdmission {
            manager_id: row
                .assigned_manager_id
                .expect("a servable environment is bound"),
        }),
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
