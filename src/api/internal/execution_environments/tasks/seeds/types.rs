//! Wire shapes for immutable task input seeds (issue 753 P2a).
//!
//! A seed request carries only the fence and the authorized files: a bounded
//! list of relative POSIX paths and their UTF-8 contents. Nothing here accepts
//! a host path, a canonical root, a Git remote, a base commit, a manager, a
//! credential or an application id, and unknown fields are denied at every
//! level so a caller cannot smuggle one in.
//!
//! Content is intentionally an authorized artifact, not log material: it
//! crosses the API because that is the point of the endpoint, and it is never
//! written to a log or an error message.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

/// Seal one immutable input snapshot for an admitted task.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
pub struct SeedNativeTaskRequest {
    #[validate(range(min = 0))]
    pub expected_generation: i64,
    /// The authorized initial files. An empty list is a valid empty workspace.
    pub files: Vec<SeedFileInput>,
}

/// One authorized file: a normalized relative POSIX path and its contents.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SeedFileInput {
    pub path: String,
    pub content: String,
}

/// The immutable descriptor of a sealed snapshot, without its contents.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct NativeTaskSeedDescriptor {
    pub task_id: Uuid,
    /// Lower-case hex SHA-256 over the versioned content manifest.
    pub manifest_sha256: String,
    pub file_count: u32,
    pub total_bytes: u32,
}

/// A sealed snapshot with its contents, for later manager consumption.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct NativeTaskSeedContent {
    pub task_id: Uuid,
    pub manifest_sha256: String,
    pub file_count: u32,
    pub total_bytes: u32,
    /// The sealed files in path order.
    pub files: Vec<SeedFileInput>,
}

impl From<crate::db::execution_environments::tasks::seeds::SeedDescriptor>
    for NativeTaskSeedDescriptor
{
    fn from(descriptor: crate::db::execution_environments::tasks::seeds::SeedDescriptor) -> Self {
        Self {
            task_id: descriptor.task_id,
            manifest_sha256: descriptor.manifest_sha256,
            file_count: descriptor.file_count,
            total_bytes: descriptor.total_bytes,
        }
    }
}

impl From<crate::db::execution_environments::tasks::seeds::SeedContent> for NativeTaskSeedContent {
    fn from(content: crate::db::execution_environments::tasks::seeds::SeedContent) -> Self {
        Self {
            task_id: content.task_id,
            manifest_sha256: content.manifest_sha256,
            file_count: content.file_count,
            total_bytes: content.total_bytes,
            files: content
                .files
                .into_iter()
                .map(|file| SeedFileInput {
                    path: file.path,
                    content: file.content,
                })
                .collect(),
        }
    }
}
