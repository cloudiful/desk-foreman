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
    /// The authorization generation this task targets.
    pub target_generation: i64,
    /// One of admitted/preparing/running/stopping/uncertain/released.
    pub state: String,
    /// The mounted container generation, once a later phase sets it.
    pub container_generation: Option<i64>,
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
            target_generation: task.target_generation,
            state: task.state.clone(),
            container_generation: task.container_generation,
            active: task.is_active(),
            released: task.is_released(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{AdmitNativeTaskRequest, CancelNativeTaskRequest, ReleaseNativeTaskRequest};

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
