//! Wire shapes for durable execution environments (issue 737 P1).
//!
//! Requests carry only the fenced inputs: `ensure` takes the opaque
//! external user plus the opaque agent-neutral runtime kind;
//! `invalidate` takes the expected generation plus an optional reason.
//! Mount paths, state-volume references, container addresses and
//! application ids are never accepted: unknown fields are denied so a
//! spoofed application or a routable environment cannot route.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

use crate::api::validation::{deserialize_optional_trimmed_nonempty, validate_non_blank};

/// Idempotent ensure request.
///
/// `external_user_id` and `runtime_kind` stay opaque to DF; the runtime
/// kind is never branched on. Application identity comes solely from the
/// bearer via `application_actor`, never from this body.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
pub struct EnsureExecutionEnvironmentRequest {
    #[validate(custom(function = "validate_non_blank"), length(min = 1, max = 128))]
    pub external_user_id: String,
    #[validate(custom(function = "validate_non_blank"), length(min = 1, max = 64))]
    pub runtime_kind: String,
}

/// Owner-scoped invalidate request.
///
/// `expected_generation` is the caller's fencing input; the update
/// commits only on an exact match. `reason` is diagnostic only.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
pub struct InvalidateExecutionEnvironmentRequest {
    #[validate(range(min = 0))]
    pub expected_generation: i64,
    #[serde(default, deserialize_with = "deserialize_optional_trimmed_nonempty")]
    #[validate(length(max = 512))]
    pub reason: Option<String>,
}

/// Async provision request. `operation_token` is a bounded idempotency key;
/// application identity, manager, image, path and endpoint are never accepted.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
pub struct ProvisionExecutionEnvironmentRequest {
    #[validate(range(min = 0))]
    pub expected_generation: i64,
    #[validate(custom(function = "validate_non_blank"), length(min = 1, max = 128))]
    pub operation_token: String,
}

/// Async stop request for one exact authorization generation.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
pub struct StopExecutionEnvironmentRequest {
    #[validate(range(min = 0))]
    pub expected_generation: i64,
    #[validate(custom(function = "validate_non_blank"), length(min = 1, max = 128))]
    pub operation_token: String,
}

/// Async replace request. The caller fences the authorization generation, the
/// provision candidate and the exact consumed stop proof job.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
pub struct ReplaceExecutionEnvironmentRequest {
    #[validate(range(min = 0))]
    pub expected_generation: i64,
    #[validate(range(min = 0))]
    pub expected_container_generation: i64,
    pub expected_stop_job_id: Uuid,
    #[validate(custom(function = "validate_non_blank"), length(min = 1, max = 128))]
    pub operation_token: String,
}
