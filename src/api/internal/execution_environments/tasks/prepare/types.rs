//! Wire shapes for durable native-task preparation (issue 753 P2b).
//!
//! A schedule request carries only the caller's generation fence and an opaque
//! operation token: the task, its binding, its manager and its seed are all
//! derived from stored state, so a caller can never name a host path, a
//! manager, an image or a credential.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

use crate::api::validation::validate_non_blank;

/// Schedule (or idempotently replay) one durable preparation.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
pub struct PrepareNativeTaskRequestApi {
    #[validate(range(min = 0))]
    pub expected_generation: i64,
    #[validate(custom(function = "validate_non_blank"), length(min = 1, max = 128))]
    pub operation_token: String,
}

/// The durable descriptor of a scheduled preparation.
///
/// Identity and status only: no host path, manager endpoint, seed content or
/// credential crosses this boundary.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct NativeTaskPreparationDescriptor {
    pub task_id: Uuid,
    /// The task's state, including `preparing` and `prepared`.
    pub task_state: String,
    pub job_id: Uuid,
    pub job_state: String,
    pub target_generation: i64,
    /// True while the task still occupies its environment and workspace.
    pub active: bool,
    pub released: bool,
}

impl From<crate::db::execution_environments::tasks::prepare::PreparationDescriptor>
    for NativeTaskPreparationDescriptor
{
    fn from(
        descriptor: crate::db::execution_environments::tasks::prepare::PreparationDescriptor,
    ) -> Self {
        Self {
            task_id: descriptor.task_id,
            task_state: descriptor.task_state,
            job_id: descriptor.job_id,
            job_state: descriptor.job_state,
            target_generation: descriptor.target_generation,
            active: descriptor.active,
            released: descriptor.released,
        }
    }
}
