use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::super::types::{NativeTask, TaskEnvironment};
use crate::db::execution_environment_jobs::ExecutionEnvironmentJob;

/// Fenced request shared by native start and stop scheduling.
#[derive(Clone, Debug)]
pub struct TaskExecutionRequest {
    pub expected_generation: i64,
    pub operation_token: String,
}

/// Public internal descriptor for one task-linked durable operation.
#[derive(Clone, Debug)]
pub struct NativeTaskOperationDescriptor {
    pub environment_id: Uuid,
    pub generation: i64,
    pub environment_state: String,
    pub task_id: Uuid,
    pub task_state: String,
    pub job_id: Uuid,
    pub job_kind: String,
    pub job_state: String,
    pub native_task_id: Uuid,
    pub(crate) stop_disposition: Option<NativeTaskStopDisposition>,
}

/// Internal relay-cancellation effect of a task stop transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NativeTaskStopDisposition {
    Admitted { cancel_through_generation: i64 },
    Replay,
}

impl NativeTaskOperationDescriptor {
    pub(crate) fn relay_cancel_through_generation(&self) -> Option<i64> {
        match self.stop_disposition {
            Some(NativeTaskStopDisposition::Admitted {
                cancel_through_generation,
            }) => Some(cancel_through_generation),
            Some(NativeTaskStopDisposition::Replay) | None => None,
        }
    }

    pub(crate) fn new(
        environment: &TaskEnvironment,
        task: &NativeTask,
        job: &ExecutionEnvironmentJob,
    ) -> Self {
        Self {
            environment_id: environment.environment_id,
            generation: environment.generation,
            environment_state: environment.state.clone(),
            task_id: task.task_id,
            task_state: task.state.clone(),
            job_id: job.job_id,
            job_kind: job.job_kind.clone(),
            job_state: job.state.clone(),
            native_task_id: task.task_id,
            stop_disposition: None,
        }
    }
}

/// Why a native start/stop intent was refused.
#[derive(Debug)]
pub enum TaskExecutionError {
    NotFound,
    StaleGeneration,
    Conflict,
    TokenConflict,
    InvalidToken,
    ManagerUnavailable,
    Internal,
}

impl TaskExecutionError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::StaleGeneration => "stale_generation",
            Self::Conflict => "conflict",
            Self::TokenConflict => "token_conflict",
            Self::InvalidToken => "invalid_token",
            Self::ManagerUnavailable => "manager_unavailable",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug)]
pub(crate) enum NativeJobCompletion {
    Completed,
    AlreadyTerminal,
    NotCompletable,
    NotFound,
    ForeignManager,
    Inconsistent(&'static str),
}

pub(crate) fn internal(error: impl std::fmt::Display) -> TaskExecutionError {
    tracing::error!(error = %error, "native task execution persistence failed");
    TaskExecutionError::Internal
}

pub(crate) fn deadline_after(seconds: i64) -> DateTime<Utc> {
    Utc::now() + chrono::Duration::seconds(seconds)
}
