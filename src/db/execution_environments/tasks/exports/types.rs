use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

use runner_protocol::NativeTaskExportValidationError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTaskExportDescriptor {
    pub task_id: Uuid,
    pub job_id: Uuid,
    pub stop_job_id: Uuid,
    pub state: String,
    pub selection_sha256: String,
    pub output_manifest_sha256: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeTaskExportError {
    NotFound,
    InvalidToken,
    InvalidSelection(NativeTaskExportValidationError),
    InvalidContent(NativeTaskExportValidationError),
    Conflict,
    ManagerUnavailable,
    NotReady,
    NotCompletable,
    Internal,
}

impl NativeTaskExportError {
    pub fn code(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::InvalidToken => "invalid_token",
            Self::InvalidSelection(error) | Self::InvalidContent(error) => error.code(),
            Self::Conflict => "conflict",
            Self::ManagerUnavailable => "manager_unavailable",
            Self::NotReady => "not_ready",
            Self::NotCompletable => "not_completable",
            Self::Internal => "internal",
        }
    }
}

pub(super) fn internal(error: impl std::fmt::Display) -> NativeTaskExportError {
    tracing::error!(error = %error, "native task export persistence failed");
    NativeTaskExportError::Internal
}

#[derive(Clone, Debug, FromRow)]
pub(super) struct TaskExportHeader {
    pub(super) job_id: Uuid,
    pub(super) task_id: Uuid,
    pub(super) environment_id: Uuid,
    pub(super) application_id: i64,
    pub(super) workspace_binding_id: i64,
    pub(super) assigned_manager_id: i64,
    pub(super) target_generation: i64,
    pub(super) stop_job_id: Uuid,
    pub(super) selection_sha256: String,
    pub(super) selection_file_count: i32,
    pub(super) output_manifest_sha256: Option<String>,
    pub(super) output_file_count: Option<i32>,
    pub(super) output_missing_count: Option<i32>,
    pub(super) output_total_bytes: Option<i64>,
    pub(super) completed_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, FromRow)]
pub(super) struct TaskExportFile {
    pub(super) path: String,
    pub(super) content: String,
}

impl TaskExportHeader {
    pub(super) fn descriptor(&self, state: &str) -> NativeTaskExportDescriptor {
        NativeTaskExportDescriptor {
            task_id: self.task_id,
            job_id: self.job_id,
            stop_job_id: self.stop_job_id,
            state: state.to_string(),
            selection_sha256: self.selection_sha256.clone(),
            output_manifest_sha256: self.output_manifest_sha256.clone(),
        }
    }
}
