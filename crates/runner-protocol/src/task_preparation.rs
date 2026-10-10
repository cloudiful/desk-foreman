//! Shared protocol DTOs for original-manager task preparation (issue 753 P2b).
//!
//! Two request/ack shapes cross the control-plane/manager boundary:
//!
//! - [`PrepareNativeTaskRequest`] is what the control plane enqueues as a
//!   durable preparation job. It carries only identity, the authorized
//!   workspace binding, and the seed's identity (hash, count, bytes) — never a
//!   host path, an image, a manager, a secret or generic parameter JSON.
//! - [`NativeTaskPreparedResult`] is the manager's typed proof that it
//!   materialized exactly that seed. Only an ack that matches every field turns
//!   a task into `prepared`; it can never mark an environment ready or set a
//!   container generation.
//!
//! The seed bytes themselves do not travel in the request: the manager fetches
//! them from the authenticated control plane and re-validates them with
//! [`super::task_seed`] before writing anything.

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::RunnerOwner;

/// Durable instruction to prepare one task's private checkout from its sealed
/// seed. Live only on the assigned (original) manager.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PrepareNativeTaskRequest {
    /// The environment owner this task belongs to, including its generation.
    pub owner: RunnerOwner,
    pub task_id: Uuid,
    pub workspace_binding_id: i64,
    /// Content digest of the sealed seed the manager must materialize.
    pub manifest_sha256: String,
    pub file_count: u32,
    pub total_bytes: u32,
}

/// The manager's typed proof that it materialized the exact sealed seed.
///
/// `prepared` is the only positive signal; a mismatch in any field is never a
/// success. No host path, file listing or secret is included.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeTaskPreparedResult {
    pub task_id: Uuid,
    pub manifest_sha256: String,
    pub file_count: u32,
    pub total_bytes: u32,
    pub prepared: bool,
}
