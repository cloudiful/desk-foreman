//! Focused lifecycle types for authenticated asynchronous environment
//! control (issue 742 P3).
//!
//! A `LifecycleEnvironment` is the environment row with its immutable manager
//! binding, provision container candidate and consumed stop proof. The
//! descriptor is the public internal lifecycle status shape: identity,
//! authorization generation, checked state, and an optional operation
//! (`job_id`/`kind`/`state`). No state volume, token, manager or host detail is
//! ever exposed.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::db::execution_environment_jobs::ExecutionEnvironmentJobResult;

/// Environment state set when a provision intent is admitted.
pub const LIFECYCLE_STATE_STARTING: &str = "starting";
/// Environment state set when a matching ensure result is reconciled.
pub const LIFECYCLE_STATE_READY: &str = "ready";
/// Environment state set when a stop intent is admitted.
pub const LIFECYCLE_STATE_STOPPING: &str = "stopping";
/// Environment state set when a confirmed stop proof is recorded.
pub const LIFECYCLE_STATE_REPLACED: &str = "replaced";
/// Environment state set when a stop is admitted but not yet confirmed.
pub const LIFECYCLE_STATE_UNCERTAIN: &str = "uncertain";

/// Checked environment states accepted by the lifecycle layer.
pub const LIFECYCLE_ENVIRONMENT_STATES: [&str; 7] = [
    "absent",
    "starting",
    "ready",
    "stopping",
    "replaced",
    "unreachable",
    "uncertain",
];

/// Typed result persisted on an obsolete ensure record superseded by a
/// confirmed stop proof. It is never an ensure success.
pub const SUPERSEDED_BY_STOP_PROOF_OUTCOME: &str = "superseded_by_stop_proof";

/// Observed states a confirmed stop result may carry.
pub const ACCEPTED_CONFIRMED_STOP_STATES: [&str; 3] = ["absent", "exited", "dead"];

/// Environment row with lifecycle metadata. `container_generation` is the
/// provision candidate (`None` before the first admitted provision);
/// `assigned_manager_id` is the immutable manager binding; the stop-proof
/// fields are the consumed stop evidence.
#[derive(Clone, Debug, FromRow)]
pub struct LifecycleEnvironment {
    pub environment_id: Uuid,
    pub application_id: i64,
    pub external_user_id: String,
    pub runtime_kind: String,
    pub generation: i64,
    pub state: String,
    pub state_volume_ref: String,
    pub assigned_manager_id: Option<i64>,
    pub container_generation: Option<i64>,
    pub stop_proof_job_id: Option<Uuid>,
    pub stop_proof_generation: Option<i64>,
    pub last_active_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl LifecycleEnvironment {
    /// The provision candidate generation, if any.
    pub fn candidate(&self) -> Option<i64> {
        self.container_generation
    }

    /// True when a manager is immutably bound.
    pub fn is_bound(&self) -> bool {
        self.assigned_manager_id.is_some()
    }
}

/// One operation reference inside the lifecycle descriptor.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, PartialEq, Eq)]
pub struct OperationDescriptor {
    pub job_id: Uuid,
    pub kind: String,
    pub state: String,
}

/// Public internal lifecycle descriptor. Async intent acknowledgment is not a
/// running or stop confirmation.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, PartialEq, Eq)]
pub struct EnvironmentOperationDescriptor {
    pub environment_id: Uuid,
    pub generation: i64,
    pub state: String,
    pub operation: Option<OperationDescriptor>,
}

/// Build the descriptor for one environment and its optional active operation.
pub fn environment_descriptor(
    environment: &LifecycleEnvironment,
    operation: Option<&OperationDescriptor>,
) -> EnvironmentOperationDescriptor {
    EnvironmentOperationDescriptor {
        environment_id: environment.environment_id,
        generation: environment.generation,
        state: environment.state.clone(),
        operation: operation.cloned(),
    }
}

/// Operation reference for one durable job row.
pub fn operation_descriptor(job_id: Uuid, kind: &str, state: &str) -> OperationDescriptor {
    OperationDescriptor {
        job_id,
        kind: kind.to_string(),
        state: state.to_string(),
    }
}

/// Parse a persisted result as a confirmed stop and return its observed
/// state, or `None` when it is not a positively confirmed stop.
pub fn confirmed_stop_state(result: Option<&serde_json::Value>) -> Option<String> {
    let value = result?;
    let parsed: ExecutionEnvironmentJobResult = serde_json::from_value(value.clone()).ok()?;
    if parsed.outcome != "stop_confirmed" || parsed.confirmed != Some(true) {
        return None;
    }
    let state = parsed.observed_state?;
    ACCEPTED_CONFIRMED_STOP_STATES
        .contains(&state.as_str())
        .then_some(state)
}

/// Parse a persisted result as a successful ensure and return `true` only for
/// an explicitly running environment.
pub fn ensure_running_result(result: Option<&serde_json::Value>) -> bool {
    let Some(value) = result else {
        return false;
    };
    let Ok(parsed) = serde_json::from_value::<ExecutionEnvironmentJobResult>(value.clone()) else {
        return false;
    };
    parsed.outcome == "ensure_running"
        && parsed
            .observed_state
            .as_deref()
            .is_some_and(|state| state == "running")
}

/// True when `state` is one of the checked lifecycle values.
pub fn lifecycle_state_is_valid(state: &str) -> bool {
    LIFECYCLE_ENVIRONMENT_STATES.contains(&state)
}

/// Build the typed superseded result recorded on an obsolete ensure row.
pub fn superseded_result() -> ExecutionEnvironmentJobResult {
    ExecutionEnvironmentJobResult {
        outcome: SUPERSEDED_BY_STOP_PROOF_OUTCOME.to_string(),
        observed_state: None,
        confirmed: None,
        preparation_quiescent: None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn lifecycle_states_are_checked_values() {
        for state in LIFECYCLE_ENVIRONMENT_STATES {
            assert!(lifecycle_state_is_valid(state), "{state} must be valid");
        }
        assert!(lifecycle_state_is_valid("uncertain"));
        for state in ["running", "STARTING", "stopped", ""] {
            assert!(
                !lifecycle_state_is_valid(state),
                "state {state} must be rejected"
            );
        }
    }

    #[test]
    fn confirmed_stop_state_accepts_only_positive_quiescence() {
        for state in ACCEPTED_CONFIRMED_STOP_STATES {
            let value = json!({
                "outcome": "stop_confirmed",
                "observed_state": state,
                "confirmed": true,
            });
            assert_eq!(
                confirmed_stop_state(Some(&value)).as_deref(),
                Some(state),
                "{state} must confirm"
            );
        }
        // `removed` and arbitrary states are not accepted confirmations.
        let removed = json!({
            "outcome": "stop_confirmed",
            "observed_state": "removed",
            "confirmed": true,
        });
        assert_eq!(confirmed_stop_state(Some(&removed)), None);
        let unconfirmed = json!({
            "outcome": "stop_unconfirmed",
            "observed_state": "running",
            "confirmed": false,
        });
        assert_eq!(confirmed_stop_state(Some(&unconfirmed)), None);
        assert_eq!(confirmed_stop_state(None), None);
    }

    #[test]
    fn ensure_running_result_requires_explicit_running() {
        let running = json!({
            "outcome": "ensure_running",
            "observed_state": "running",
        });
        assert!(ensure_running_result(Some(&running)));
        let not_running = json!({
            "outcome": "ensure_running",
            "observed_state": "exited",
        });
        assert!(!ensure_running_result(Some(&not_running)));
        let uncertain = json!({
            "outcome": "ensure_uncertain",
        });
        assert!(!ensure_running_result(Some(&uncertain)));
        assert!(!ensure_running_result(None));
    }

    #[test]
    fn superseded_result_is_never_a_success() {
        let result = superseded_result();
        assert_eq!(result.outcome, SUPERSEDED_BY_STOP_PROOF_OUTCOME);
        assert!(result.confirmed.is_none());
        assert!(result.observed_state.is_none());
        assert!(!ensure_running_result(Some(
            &serde_json::to_value(&result).unwrap()
        )));
    }
}
