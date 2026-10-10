//! Preparation types for durable native-task jobs (issue 753 P2b).
//!
//! A preparation is a durable job that instructs the task's *original* manager
//! to materialize the task's sealed seed into a private checkout and return a
//! typed proof. Nothing here starts a container, mounts anything or writes a
//! canonical root.

use uuid::Uuid;

/// Why a preparation was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrepareError {
    /// The environment, task or binding is absent for this application.
    NotFound,
    /// The caller's generation fence does not match the stored generation.
    StaleGeneration,
    /// The task is not the environment's active admitted intent.
    TaskNotAdmitted,
    /// The task's binding is not a live code project resource workspace.
    BindingIneligible,
    /// The subject does not match the environment's own subject.
    SubjectMismatch,
    /// The task has no sealed input snapshot to materialize.
    SeedMissing,
    /// A candidate container generation exists or the environment is not idle.
    EnvironmentNotIdle,
    /// The task's original manager is missing, disabled or offline.
    ManagerUnavailable,
    /// A lifecycle operation is unresolved for this environment.
    UnresolvedBlockers,
    /// The same operation token was used with a different intent.
    TokenConflict,
    /// A query failed; the text is never surfaced to a caller.
    Internal,
}

impl PrepareError {
    /// Stable, prose-free code for assertions and logs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::StaleGeneration => "stale_generation",
            Self::TaskNotAdmitted => "task_not_admitted",
            Self::BindingIneligible => "binding_ineligible",
            Self::SubjectMismatch => "subject_mismatch",
            Self::SeedMissing => "seed_missing",
            Self::EnvironmentNotIdle => "environment_not_idle",
            Self::ManagerUnavailable => "manager_unavailable",
            Self::UnresolvedBlockers => "unresolved_blockers",
            Self::TokenConflict => "token_conflict",
            Self::Internal => "internal",
        }
    }
}

/// The durable descriptor of a scheduled preparation.
///
/// Identity and status only: the task, the preparation job, its state and the
/// environment generation it targets. No host path, manager endpoint or seed
/// content is ever part of it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparationDescriptor {
    pub task_id: Uuid,
    pub task_state: String,
    pub job_id: Uuid,
    pub job_state: String,
    pub target_generation: i64,
    /// True while the task still occupies its environment and workspace.
    pub active: bool,
    /// True once the task has reached its terminal state.
    pub released: bool,
}

/// What one caller wants to schedule.
#[derive(Clone, Debug)]
pub struct PrepareRequest {
    pub expected_generation: i64,
    pub operation_token: String,
}

/// Wrap a query failure as an internal error without leaking its text.
pub(super) fn internal(error: impl std::fmt::Display) -> PrepareError {
    tracing::error!(error = %error, "native task preparation persistence failed");
    PrepareError::Internal
}
