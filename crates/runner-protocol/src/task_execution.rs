//! Typed native task execution protocol (issue 764 P1).
//!
//! Requests carry the task's immutable runtime identity and its environment
//! owner. They never accept caller-selected paths, images, commands or URLs.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::{EnsureExecutionEnvironmentAck, RunnerOwner, StopExecutionEnvironmentAck};

/// Immutable task and preparation identity required to execute a task.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeTaskRuntimeIdentity {
    pub task_id: Uuid,
    pub workspace_binding_id: i64,
    pub manifest_sha256: String,
    pub preparation_generation: i64,
}

/// Start one prepared task on its assigned manager.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StartNativeTaskRequest {
    pub owner: RunnerOwner,
    pub task: NativeTaskRuntimeIdentity,
}

/// Stop one task and positively prove both environment and preparation
/// quiescence before the control plane can release its workspace.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StopNativeTaskRequest {
    pub owner: RunnerOwner,
    pub task: NativeTaskRuntimeIdentity,
}

/// Successful start result. The nested environment acknowledgment uses the
/// existing owner/container/running contract.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StartNativeTaskResult {
    pub task: NativeTaskRuntimeIdentity,
    pub environment: EnsureExecutionEnvironmentAck,
}

/// Stop result. `preparation_quiescent` is mandatory and must be true before a
/// native stop can become a release proof.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StopNativeTaskResult {
    pub task: NativeTaskRuntimeIdentity,
    pub environment: StopExecutionEnvironmentAck,
    pub preparation_quiescent: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> NativeTaskRuntimeIdentity {
        NativeTaskRuntimeIdentity {
            task_id: Uuid::nil(),
            workspace_binding_id: 12,
            manifest_sha256: "a".repeat(64),
            preparation_generation: 4,
        }
    }

    fn owner() -> RunnerOwner {
        RunnerOwner::ExecutionEnvironment {
            environment_id: "123e4567-e89b-12d3-a456-426614174000".to_string(),
            generation: 4,
        }
    }

    #[test]
    fn requests_and_results_reject_unowned_execution_inputs() {
        let mut start = serde_json::to_value(StartNativeTaskRequest {
            owner: owner(),
            task: identity(),
        })
        .unwrap();
        start["host_path"] = serde_json::json!("/etc");
        assert!(serde_json::from_value::<StartNativeTaskRequest>(start).is_err());

        let mut stop = serde_json::to_value(StopNativeTaskRequest {
            owner: owner(),
            task: identity(),
        })
        .unwrap();
        stop["image"] = serde_json::json!("untrusted");
        assert!(serde_json::from_value::<StopNativeTaskRequest>(stop).is_err());
    }

    #[test]
    fn native_stop_quiescence_is_required_in_the_result_shape() {
        let value = serde_json::json!({
            "task": identity(),
            "environment": {
                "owner": owner(),
                "container_name": "df-env",
                "confirmed": true,
                "observed_state": "exited"
            }
        });
        assert!(serde_json::from_value::<StopNativeTaskResult>(value).is_err());
    }
}
