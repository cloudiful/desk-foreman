//! Immutable task input seed types for the control plane (issue 753 P2b).
//!
//! The bounds, the path rules and the manifest digest are **not** defined
//! here: they live in `runner_protocol::task_seed` so the API side and the
//! runner manager validate and hash a seed with exactly one implementation.
//! This module only re-exports those rules under the names the P2a API already
//! uses, and adds the persistence-specific row shapes.

use sqlx::FromRow;
use uuid::Uuid;

pub use runner_protocol::{
    MAX_SEED_CONTENT_BYTES, MAX_SEED_ENTRIES, MAX_SEED_PATH_BYTES, MAX_SEED_TOTAL_BYTES,
};

/// One authorized file. Alias of the shared protocol type so the wire shape,
/// the validator and the digest can never disagree about a field.
pub type SeedFile = runner_protocol::SourceSeedSnapshotFile;

/// A validated, canonically ordered snapshot. Alias of the shared protocol
/// bundle, so holding one is the same proof the manager relies on.
pub type SeedPlan = runner_protocol::SeedBundle;

/// Why a seed request was refused by content rules.
pub type SeedBoundsError = runner_protocol::SeedBundleError;

/// The immutable descriptor of a sealed snapshot.
#[derive(Clone, Debug)]
pub struct SeedDescriptor {
    pub task_id: Uuid,
    pub manifest_sha256: String,
    pub file_count: u32,
    pub total_bytes: u32,
}

/// A sealed snapshot with its contents.
#[derive(Clone, Debug)]
pub struct SeedContent {
    pub task_id: Uuid,
    pub manifest_sha256: String,
    pub file_count: u32,
    pub total_bytes: u32,
    pub files: Vec<SeedFile>,
}

/// One stored seed header row.
#[derive(Clone, Debug, FromRow)]
pub(super) struct TaskSeedRow {
    pub(super) task_id: Uuid,
    pub(super) manifest_sha256: String,
    pub(super) file_count: i64,
    pub(super) total_bytes: i64,
}

impl TaskSeedRow {
    pub(super) fn into_descriptor(self) -> SeedDescriptor {
        SeedDescriptor {
            task_id: self.task_id,
            manifest_sha256: self.manifest_sha256,
            file_count: u32::try_from(self.file_count).unwrap_or_default(),
            total_bytes: u32::try_from(self.total_bytes).unwrap_or_default(),
        }
    }
}

/// Why a seed mutation was refused against stored state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeedError {
    /// The environment or task is absent for this application.
    NotFound,
    /// The caller's generation fence does not match the stored generation.
    StaleGeneration,
    /// The task is not the environment's active task, or is already terminal.
    TaskNotAdmitted,
    /// A candidate container generation already exists.
    CandidatePresent,
    /// A durable job already references this task.
    JobReferencePresent,
    /// The task's binding is not a live code project workspace.
    BindingIneligible,
    /// A snapshot already exists and the supplied content differs.
    DifferentContent,
    /// A query failed; the text is never surfaced to a caller.
    Internal,
}

impl SeedError {
    /// Stable, prose-free code for assertions and logs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::StaleGeneration => "stale_generation",
            Self::TaskNotAdmitted => "task_not_admitted",
            Self::CandidatePresent => "candidate_present",
            Self::JobReferencePresent => "job_reference_present",
            Self::BindingIneligible => "binding_ineligible",
            Self::DifferentContent => "different_content",
            Self::Internal => "internal",
        }
    }
}

/// Wrap a query failure as an internal error without leaking its text.
///
/// The detail is logged; the caller sees a bare `Internal`, so a database
/// message can never reach a response body.
pub(super) fn internal(error: impl std::fmt::Display) -> SeedError {
    tracing::error!(error = %error, "native task seed persistence failed");
    SeedError::Internal
}
