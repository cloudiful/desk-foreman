//! Typed native task execution protocol (issue 764 P1).
//!
//! Requests carry the task's immutable runtime identity and its environment
//! owner. They never accept caller-selected paths, images, commands or URLs.
//!
//! `native_task_workspace_directory` is the one definition of where a native
//! checkout lives inside its container: the manager launch, the reuse/stop
//! mount guards, the same-container writability probe and the control-plane
//! descriptor all derive it from the stored workspace binding id, so they
//! cannot drift apart (issue 822 P1).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::{EnsureExecutionEnvironmentAck, RunnerOwner, StopExecutionEnvironmentAck};

/// Container-side root of every native task checkout.
///
/// This is a root guard, never a checkout: one task's checkout lives in its
/// own binding-scoped directory below it, and the durable state mount must
/// stay disjoint from this whole root.
pub const NATIVE_TASK_WORKSPACE_ROOT: &str = "/workspace";

/// Stable container directory for the native task checkout of one workspace
/// binding.
///
/// The path is derived from the server-owned `workspace_binding_id` alone, so
/// every task of one binding resolves to the same location while two bindings
/// never share one. It is a normative container path: it says where the
/// manager mounts a checkout, not that any container is running or reachable.
/// An `i64` renders as bounded decimal digits, so the result can never carry a
/// separator, a parent segment or a host path.
pub fn native_task_workspace_directory(workspace_binding_id: i64) -> String {
    format!("{NATIVE_TASK_WORKSPACE_ROOT}/bindings/{workspace_binding_id}")
}

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

    #[test]
    fn one_binding_keeps_one_directory_across_its_tasks() {
        let first = identity();
        let second = NativeTaskRuntimeIdentity {
            task_id: Uuid::from_u128(9),
            ..identity()
        };

        assert_eq!(
            native_task_workspace_directory(first.workspace_binding_id),
            native_task_workspace_directory(second.workspace_binding_id)
        );
        assert_eq!(
            native_task_workspace_directory(42),
            "/workspace/bindings/42"
        );
    }

    #[test]
    fn distinct_bindings_never_share_a_directory() {
        assert_ne!(
            native_task_workspace_directory(42),
            native_task_workspace_directory(7)
        );
    }

    #[test]
    fn the_directory_is_a_bounded_child_of_the_native_workspace_root() {
        for binding_id in [i64::MIN, -1, 0, 1, i64::MAX] {
            let directory = native_task_workspace_directory(binding_id);
            assert_eq!(
                directory,
                format!("/workspace/bindings/{binding_id}"),
                "the helper renders the binding id verbatim and nothing else"
            );
            assert!(directory.starts_with("/workspace/bindings/"));
            let segment = directory.trim_start_matches("/workspace/bindings/");
            assert!(
                segment == binding_id.to_string() && segment.parse::<i64>().is_ok(),
                "one integer id can never inject a path segment: {directory}"
            );
            assert!(
                !segment.is_empty() && !segment.contains(['/', '\\', '.']),
                "no separator, parent segment or relative marker survives: {directory}"
            );
        }
        assert_ne!(
            native_task_workspace_directory(42),
            NATIVE_TASK_WORKSPACE_ROOT,
            "a checkout is never the shared root itself"
        );
    }
}
