//! Wire shapes for durable native task intents (issue 753 P1).
//!
//! A task request carries only the fenced inputs: the caller-known task UUID,
//! the workspace binding it is authorized against, the subject the
//! environment belongs to, the current generation fence, and an opaque
//! operation token.
//!
//! The task UUID is an identifier, not a credential. Nothing here accepts a
//! host path, an image, a manager, a port, a state volume, a credential or an
//! application id: unknown fields are denied so a spoofed application or a
//! caller-chosen target can never route.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

use crate::api::validation::validate_non_blank;

/// Admit one durable native task intent.
///
/// `workspace_binding_id` must name a live code project resource workspace
/// owned by the authenticated application, and `subject_external_user_id` must
/// match the environment's own subject. The resource binding's fixed external
/// user marker is never compared against this value.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
pub struct AdmitNativeTaskRequest {
    pub task_id: Uuid,
    pub workspace_binding_id: i64,
    #[validate(custom(function = "validate_non_blank"), length(min = 1, max = 128))]
    pub subject_external_user_id: String,
    #[validate(range(min = 0))]
    pub expected_generation: i64,
    #[validate(custom(function = "validate_non_blank"), length(min = 1, max = 128))]
    pub operation_token: String,
}

/// Cancel one task intent at the caller's current generation fence.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
pub struct CancelNativeTaskRequest {
    #[validate(range(min = 0))]
    pub expected_generation: i64,
}

/// Release an executed task against its own confirmed stop proof.
///
/// Every field is a fence, not an instruction: the proof must belong to this
/// task, its manager and its generation, and must record positive quiescence.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
pub struct ReleaseNativeTaskRequest {
    #[validate(range(min = 0))]
    pub expected_generation: i64,
    #[validate(range(min = 0))]
    pub expected_container_generation: i64,
    pub expected_stop_job_id: Uuid,
}

/// Schedule or replay a task-aware start or stop operation.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
pub struct TaskExecutionRequestApi {
    #[validate(range(min = 0))]
    pub expected_generation: i64,
    #[validate(custom(function = "validate_non_blank"), length(min = 1, max = 128))]
    pub operation_token: String,
}

/// The task-linked durable execution operation returned by start/stop.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct NativeTaskExecutionDescriptor {
    pub environment_id: Uuid,
    pub generation: i64,
    pub environment_state: String,
    pub task_id: Uuid,
    pub task_state: String,
    pub job_id: Uuid,
    pub job_kind: String,
    pub job_state: String,
    pub native_task_id: Uuid,
}

impl From<crate::db::execution_environments::NativeTaskOperationDescriptor>
    for NativeTaskExecutionDescriptor
{
    fn from(value: crate::db::execution_environments::NativeTaskOperationDescriptor) -> Self {
        Self {
            environment_id: value.environment_id,
            generation: value.generation,
            environment_state: value.environment_state,
            task_id: value.task_id,
            task_state: value.task_state,
            job_id: value.job_id,
            job_kind: value.job_kind,
            job_state: value.job_state,
            native_task_id: value.native_task_id,
        }
    }
}

/// What one task API returns.
///
/// Identity, binding, generation and state only: no host path, state volume,
/// container, credential, manager endpoint or filesystem detail ever crosses
/// this boundary.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct NativeTaskDescriptor {
    /// Stable task owner. This, not a generation, is the writer identity.
    pub task_id: Uuid,
    pub environment_id: Uuid,
    pub workspace_binding_id: i64,
    /// The directory this task's checkout uses inside its container.
    ///
    /// A normative container path derived from `workspace_binding_id` by the
    /// same helper the manager uses to mount and to verify the checkout, so a
    /// consumer can key durable per-workspace state on it. It says nothing
    /// about a container currently running or about any manager address.
    pub workspace_directory: String,
    /// The authorization generation this task targets.
    pub target_generation: i64,
    /// Checked task state, including preparation and execution transitions.
    pub state: String,
    /// The candidate container generation, once start is admitted.
    pub container_generation: Option<i64>,
    /// Durable execution operation anchors.
    pub start_job_id: Option<Uuid>,
    pub stop_job_id: Option<Uuid>,
    /// True while this task still occupies its environment and binding.
    pub active: bool,
    /// Whether this task has reached its terminal state.
    pub released: bool,
}

impl NativeTaskDescriptor {
    /// Project one stored row into its wire shape.
    pub fn from_task(task: &crate::db::execution_environments::NativeTask) -> Self {
        Self {
            task_id: task.task_id,
            environment_id: task.environment_id,
            workspace_binding_id: task.workspace_binding_id,
            workspace_directory: runner_protocol::native_task_workspace_directory(
                task.workspace_binding_id,
            ),
            target_generation: task.target_generation,
            state: task.state.clone(),
            container_generation: task.container_generation,
            start_job_id: task.start_job_id,
            stop_job_id: task.stop_job_id,
            active: task.is_active(),
            released: task.is_released(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        AdmitNativeTaskRequest, CancelNativeTaskRequest, NativeTaskDescriptor,
        ReleaseNativeTaskRequest, TaskExecutionRequestApi,
    };

    fn stored_task(workspace_binding_id: i64) -> crate::db::execution_environments::NativeTask {
        crate::db::execution_environments::NativeTask {
            task_id: uuid::Uuid::from_u128(7),
            operation_token: "op-1".to_string(),
            environment_id: uuid::Uuid::from_u128(1),
            application_id: 1,
            workspace_binding_id,
            subject_external_user_id: "user-1".to_string(),
            target_generation: 4,
            assigned_manager_id: 7,
            state: "prepared".to_string(),
            container_generation: None,
            prepare_job_id: None,
            start_job_id: None,
            stop_job_id: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            terminal_at: None,
        }
    }

    #[test]
    fn the_descriptor_directory_follows_the_stored_binding_only() {
        let first = NativeTaskDescriptor::from_task(&stored_task(42));
        assert_eq!(first.workspace_directory, "/workspace/bindings/42");
        assert_eq!(first.workspace_binding_id, 42);

        let second = NativeTaskDescriptor::from_task(&stored_task(7));
        assert_eq!(second.workspace_directory, "/workspace/bindings/7");
        assert_ne!(first.workspace_directory, second.workspace_directory);

        assert_eq!(
            NativeTaskDescriptor::from_task(&stored_task(42)).workspace_directory,
            first.workspace_directory,
            "a later task of one binding reuses its checkout location"
        );

        let value = serde_json::to_value(&first).expect("descriptor JSON");
        assert_eq!(value["workspace_directory"], "/workspace/bindings/42");
    }

    #[test]
    fn an_admit_request_denies_unknown_fields() {
        let parsed: Result<AdmitNativeTaskRequest, _> = serde_json::from_value(json!({
            "task_id": "123e4567-e89b-12d3-a456-426614174000",
            "workspace_binding_id": 1,
            "subject_external_user_id": "user-1",
            "expected_generation": 4,
            "operation_token": "op-1",
            "workspace_root": "/etc"
        }));
        assert!(
            parsed.is_err(),
            "a caller-chosen host path must not be accepted"
        );
    }

    #[test]
    fn a_cancel_request_denies_unknown_fields() {
        let parsed: Result<CancelNativeTaskRequest, _> = serde_json::from_value(json!({
            "expected_generation": 4,
            "image": "alpine"
        }));
        assert!(parsed.is_err(), "an image must never be accepted");
    }

    #[test]
    fn a_task_execution_request_denies_runtime_configuration() {
        let parsed: Result<TaskExecutionRequestApi, _> = serde_json::from_value(json!({
            "expected_generation": 4,
            "operation_token": "op-1",
            "host_path": "/etc",
            "command": "unsafe"
        }));
        assert!(parsed.is_err());
    }

    #[test]
    fn a_release_request_denies_unknown_fields() {
        let parsed: Result<ReleaseNativeTaskRequest, _> = serde_json::from_value(json!({
            "expected_generation": 4,
            "expected_container_generation": 4,
            "expected_stop_job_id": "123e4567-e89b-12d3-a456-426614174000",
            "manager_id": 7
        }));
        assert!(
            parsed.is_err(),
            "a caller must never name the manager that ran the stop"
        );
    }
}
