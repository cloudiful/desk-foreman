//! Agent-neutral execution environment runner primitives (issue 739 P1).
//!
//! Focused DTOs and pure helpers for isolated persistent-state
//! environments. No container start/stop, routing, or production
//! migration lives here: this module only shapes requests, acknowledgments,
//! stable naming, and fail-closed validation. Runtime behavior stays in
//! `crates/runner-manager`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::RunnerOwner;

/// Marker state returned by a successful ensure.
pub const EXECUTION_ENVIRONMENT_RUNNING_STATE: &str = "running";

/// States that prove a container no longer holds durable state.
pub const EXECUTION_ENVIRONMENT_STOPPED_STATES: [&str; 3] = ["exited", "dead", "removed"];

/// Explicitly absent after a positively identified missing container.
pub const EXECUTION_ENVIRONMENT_ABSENT_STATE: &str = "absent";

/// Idempotent ensure request. Only the owner travels; image, host paths,
/// volumes, env, secrets, and endpoints are never accepted from requests.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EnsureExecutionEnvironmentRequest {
    pub owner: RunnerOwner,
}

/// Typed acknowledgment for a successful ensure. Echoes the request owner
/// (including generation) with the observed container state. No secrets,
/// host paths, or volume details.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EnsureExecutionEnvironmentAck {
    pub owner: RunnerOwner,
    pub container_name: String,
    pub state: String,
}

/// Stop request for one exact generation. Only the owner travels.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StopExecutionEnvironmentRequest {
    pub owner: RunnerOwner,
}

/// Stop acknowledgment. `confirmed` is true only after a successful
/// inspect proves `exited`/`dead`/`removed` or positively identified
/// absence. Transport, parse, timeout, or permission failures never
/// confirm. No secrets, host paths, or volume details.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StopExecutionEnvironmentAck {
    pub owner: RunnerOwner,
    pub container_name: String,
    pub confirmed: bool,
    pub observed_state: String,
}

/// Borrowed view of an execution-environment owner.
pub fn as_execution_environment(owner: &RunnerOwner) -> Option<(&str, i64)> {
    match owner {
        RunnerOwner::ExecutionEnvironment {
            environment_id,
            generation,
        } => Some((environment_id.as_str(), *generation)),
        _ => None,
    }
}

/// True when the owner is an execution environment.
pub fn is_execution_environment(owner: &RunnerOwner) -> bool {
    as_execution_environment(owner).is_some()
}

/// Validate an environment UUID string (hyphenated `8-4-4-4-12` hex).
pub fn validate_environment_id(environment_id: &str) -> Result<(), String> {
    if environment_id.len() != 36 {
        return Err(format!(
            "invalid execution environment id: expected UUID, got {environment_id:?}"
        ));
    }
    let bytes = environment_id.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        let expected_hyphen = matches!(index, 8 | 13 | 18 | 23);
        if expected_hyphen {
            if *byte != b'-' {
                return Err(format!(
                    "invalid execution environment id: expected UUID, got {environment_id:?}"
                ));
            }
        } else if !byte.is_ascii_hexdigit() {
            return Err(format!(
                "invalid execution environment id: expected UUID, got {environment_id:?}"
            ));
        }
    }
    Ok(())
}

/// Validate a full environment owner: variant, UUID shape, generation bound.
pub fn validate_environment_owner(owner: &RunnerOwner) -> Result<(&str, i64), String> {
    let (environment_id, generation) = as_execution_environment(owner)
        .ok_or_else(|| "runner owner is not an execution environment".to_string())?;
    validate_environment_id(environment_id)?;
    if generation < 0 {
        return Err(format!(
            "invalid execution environment generation: expected >= 0, got {generation}"
        ));
    }
    Ok((environment_id, generation))
}

/// Environment-level lock key: solely the environment id, never the
/// generation. Generation-specific locks cannot serialize replacement.
pub fn environment_lock_key(environment_id: &str) -> String {
    format!("execution_environment:{environment_id}")
}

/// Stable Docker volume name derived from the environment UUID only, so it
/// stays identical across generations. Lowercase hyphenless form.
pub fn derive_state_volume_name(environment_id: &str) -> Result<String, String> {
    validate_environment_id(environment_id)?;
    let simple: String = environment_id
        .chars()
        .filter(|c| *c != '-')
        .collect::<String>()
        .to_ascii_lowercase();
    Ok(format!("df-env-{simple}"))
}

/// True when a Docker inspect `State.Status` proves the container stopped.
pub fn is_stopped_status(status: &str) -> bool {
    EXECUTION_ENVIRONMENT_STOPPED_STATES.contains(&status)
}

/// True when a Docker inspect status means another generation still holds
/// state (running-like or unrecognized). Only `exited`/`dead`/`removed`
/// prove quiescence; everything else refuses creation.
pub fn is_running_or_unknown_status(status: &str) -> bool {
    !is_stopped_status(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(id: &str, generation: i64) -> RunnerOwner {
        RunnerOwner::ExecutionEnvironment {
            environment_id: id.to_string(),
            generation,
        }
    }

    #[test]
    fn validates_uuid_and_generation_bounds() {
        let good = "123e4567-e89b-12d3-a456-426614174000";
        assert!(validate_environment_owner(&owner(good, 0)).is_ok());
        assert!(validate_environment_owner(&owner(good, 7)).is_ok());
        assert!(validate_environment_owner(&owner("not-a-uuid", 0)).is_err());
        assert!(validate_environment_owner(&owner("", 0)).is_err());
        assert!(validate_environment_owner(&owner(good, -1)).is_err());
        assert!(validate_environment_owner(&RunnerOwner::InternalUser { user_id: 1 }).is_err());
    }

    #[test]
    fn volume_name_is_stable_across_generations() {
        let id = "123E4567-E89B-12D3-A456-426614174000";
        let first = derive_state_volume_name(id).expect("valid uuid");
        let second = derive_state_volume_name(&id.to_ascii_lowercase()).expect("valid uuid");
        assert_eq!(first, second);
        assert_eq!(first, "df-env-123e4567e89b12d3a456426614174000");
    }

    #[test]
    fn stopped_states_are_exact() {
        assert!(is_stopped_status("exited"));
        assert!(is_stopped_status("dead"));
        assert!(is_stopped_status("removed"));
        assert!(!is_stopped_status("running"));
        assert!(!is_stopped_status("unknown"));
        assert!(is_running_or_unknown_status("running"));
        assert!(is_running_or_unknown_status("unknown"));
        assert!(!is_running_or_unknown_status("exited"));
    }

    #[test]
    fn requests_reject_unknown_fields() {
        let good = "123e4567-e89b-12d3-a456-426614174000";
        let payload = serde_json::json!({
            "owner": {"ExecutionEnvironment": {"environment_id": good, "generation": 2}},
            "image": "evil:latest"
        });
        assert!(serde_json::from_value::<EnsureExecutionEnvironmentRequest>(payload).is_err());
        let payload = serde_json::json!({
            "owner": {"ExecutionEnvironment": {"environment_id": good, "generation": 2}},
            "host_path": "/etc"
        });
        assert!(serde_json::from_value::<StopExecutionEnvironmentRequest>(payload).is_err());
    }
}
