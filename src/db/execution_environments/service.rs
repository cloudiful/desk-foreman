//! Owner-scoped persistence for the private agent-service gateway
//! (issue 749 P2b).
//!
//! Admission needs a small, fixed projection of the environment row and
//! nothing else. Reading it through one owner-scoped statement means the
//! gateway cannot accidentally widen its view to another application, and
//! it cannot accidentally start selecting columns that carry external-user
//! or volume detail.

use sqlx::PgPool;
use uuid::Uuid;

/// The only environment facts service admission is allowed to see.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct ServiceEnvironment {
    /// Application-authorized generation; the caller's header must match it.
    pub generation: i64,
    /// Provisioned container generation; the header must match it too.
    pub container_generation: Option<i64>,
    /// Immutable manager binding. `None` means no manager may serve.
    pub assigned_manager_id: Option<i64>,
    /// Checked lifecycle state.
    pub state: String,
}

impl ServiceEnvironment {
    /// True when this environment may serve right now: it is ready, a
    /// manager is immutably bound, and the authorized generation and the
    /// provisioned candidate agree.
    ///
    /// The caller must additionally require `generation == header`, so the
    /// presented fence is checked against the same stored value here.
    pub fn is_servable(&self, presented_generation: i64) -> bool {
        self.state == crate::db::execution_environments::LIFECYCLE_STATE_READY
            && self.assigned_manager_id.is_some()
            && self.generation == presented_generation
            && self.container_generation == Some(presented_generation)
    }

    /// True when the row is servable in principle but no manager tunnel
    /// could currently be reached. Callers map this to 503, not 409, so a
    /// temporary relay outage is never reported as a client conflict.
    pub fn is_bound_but_not_servable(&self, presented_generation: i64) -> bool {
        self.assigned_manager_id.is_some() && self.generation == presented_generation
    }
}

/// Look up one environment for service admission, scoped to its owner.
///
/// `None` covers both a foreign environment and an unknown id, so the
/// gateway cannot distinguish "does not exist" from "belongs to someone
/// else" and cannot be used as an existence oracle.
pub async fn find_execution_environment_for_service(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
) -> anyhow::Result<Option<ServiceEnvironment>> {
    let row = sqlx::query_file_as!(
        ServiceEnvironment,
        "src/sql/execution_environments/find_execution_environment_for_service.sql",
        environment_id,
        application_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

#[cfg(test)]
mod admission_tests {
    use super::ServiceEnvironment;

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
    fn a_ready_bound_environment_with_matching_fences_serves() {
        assert!(env(4, Some(4), Some(7), "ready").is_servable(4));
    }

    #[test]
    fn a_presented_generation_must_match_both_fences() {
        // Authorized generation is the stored counter the header is checked
        // against; the candidate is the provisioned container generation.
        assert!(!env(4, Some(4), Some(7), "ready").is_servable(3));
        assert!(!env(4, Some(5), Some(7), "ready").is_servable(4));
        assert!(
            !env(4, None, Some(7), "ready").is_servable(4),
            "an environment with no provisioned candidate has nothing to serve"
        );
    }

    #[test]
    fn an_unbound_or_unready_environment_never_serves() {
        assert!(!env(4, Some(4), None, "ready").is_servable(4));
        for state in [
            "starting",
            "stopping",
            "replaced",
            "absent",
            "unreachable",
            "uncertain",
        ] {
            assert!(
                !env(4, Some(4), Some(7), state).is_servable(4),
                "{state} must not serve"
            );
        }
    }

    #[test]
    fn a_bound_environment_stays_distinguishable_from_an_unusable_one() {
        // A relay outage is 503, not 409, so callers must be able to tell
        // "wrong fence or unbound" from "correctly fenced but unreachable".
        assert!(env(4, Some(4), Some(7), "ready").is_bound_but_not_servable(4));
        assert!(!env(4, Some(4), None, "ready").is_bound_but_not_servable(4));
        assert!(!env(4, Some(4), Some(7), "ready").is_bound_but_not_servable(3));
    }
}
