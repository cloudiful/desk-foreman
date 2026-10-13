use std::path::PathBuf;

use serde_json::Value;
use utoipa::ToSchema;

pub mod execution_environment;
pub mod service_relay;
pub mod task_execution;
pub mod task_export;
pub mod task_preparation;
pub mod task_seed;

pub use execution_environment::{
    EnsureExecutionEnvironmentAck, EnsureExecutionEnvironmentRequest, StopExecutionEnvironmentAck,
    StopExecutionEnvironmentRequest,
};
pub use task_execution::{
    NATIVE_TASK_WORKSPACE_ROOT, NativeTaskRuntimeIdentity, StartNativeTaskRequest,
    StartNativeTaskResult, StopNativeTaskRequest, StopNativeTaskResult,
    native_task_workspace_directory,
};
pub use task_export::{
    ExportNativeTaskRequest, ExportNativeTaskResult, ExportNativeTaskUpload,
    MAX_TASK_EXPORT_PATH_BYTES, MAX_TASK_EXPORT_PATHS, MAX_TASK_EXPORT_UPLOAD_BODY_BYTES,
    NativeTaskExportContent, NativeTaskExportSelection, NativeTaskExportValidationError,
    ValidatedNativeTaskExport, native_task_export_output_manifest_sha256,
    selection_manifest_sha256, validate_native_task_export_content,
    validate_native_task_export_selection,
};
pub use task_preparation::{NativeTaskPreparedResult, PrepareNativeTaskRequest};
pub use task_seed::{
    MAX_SEED_CONTENT_BYTES, MAX_SEED_ENTRIES, MAX_SEED_PATH_BYTES, MAX_SEED_TOTAL_BYTES,
    SEED_MANIFEST_PREFIX, SeedBundle, SeedBundleError, SourceSeedSnapshotFile,
    is_normalized_relative_posix, manifest_sha256, validate_seed_bundle,
};
pub const RUNNER_JOB_TIMEOUT_SECS: u64 = 3_660;
pub const RUNNER_JOB_POLL_TIMEOUT_SECS: u64 = 10;
pub const RUNNER_MANAGER_HEARTBEAT_TTL_SECS: u64 = 30;

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunnerLifecycleStatus {
    Running,
    Removed,
    CleanupFailed,
}

impl RunnerLifecycleStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Removed => "removed",
            Self::CleanupFailed => "cleanup_failed",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize, ToSchema)]
pub struct RunnerLifecycleEvent {
    pub owner: RunnerOwner,
    pub container_name: String,
    pub container_id: Option<String>,
    pub status: RunnerLifecycleStatus,
    pub workspace_root: Option<String>,
    pub runtime: Option<String>,
    pub runtime_class: Option<String>,
    pub image_name: Option<String>,
    pub network_enabled: Option<bool>,
    pub last_error: Option<String>,
}

#[cfg(test)]
mod lifecycle_tests {
    use super::RunnerLifecycleStatus;

    #[test]
    fn lifecycle_statuses_use_database_names() {
        assert_eq!(RunnerLifecycleStatus::Running.as_str(), "running");
        assert_eq!(RunnerLifecycleStatus::Removed.as_str(), "removed");
        assert_eq!(
            RunnerLifecycleStatus::CleanupFailed.as_str(),
            "cleanup_failed"
        );
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize, ToSchema)]
pub enum RunnerOwner {
    InternalUser {
        user_id: i64,
    },
    WorkspaceBinding {
        workspace_binding_id: i64,
    },
    ExecutionEnvironment {
        environment_id: String,
        generation: i64,
    },
}

impl RunnerOwner {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InternalUser { .. } => "user",
            Self::WorkspaceBinding { .. } => "workspace_binding",
            Self::ExecutionEnvironment { .. } => "execution_environment",
        }
    }

    pub fn stable_key(&self) -> String {
        match self {
            Self::InternalUser { user_id } => format!("user:{user_id}"),
            Self::WorkspaceBinding {
                workspace_binding_id,
            } => format!("workspace_binding:{workspace_binding_id}"),
            Self::ExecutionEnvironment {
                environment_id,
                generation,
            } => format!("execution_environment:{environment_id}:{generation}"),
        }
    }

    /// In-process lock key. Environment owners serialize on the
    /// environment id alone; generation-specific locks cannot protect
    /// replacement. Legacy owners keep their stable key.
    pub fn lock_key(&self) -> String {
        match self {
            Self::ExecutionEnvironment { environment_id, .. } => {
                crate::execution_environment::environment_lock_key(environment_id)
            }
            _ => self.stable_key(),
        }
    }

    pub fn container_name(&self) -> String {
        format!(
            "desk-foreman-runner-{}",
            self.stable_key().replace([':', '/'], "-")
        )
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct RunnerShellRequest {
    pub owner: RunnerOwner,
    pub workspace_root: PathBuf,
    pub working_dir: PathBuf,
    pub shell: String,
    pub login: bool,
    pub tty: bool,
    pub command: String,
    pub network_enabled: bool,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct RunnerCommandRequest {
    pub owner: RunnerOwner,
    pub workspace_root: PathBuf,
    pub working_dir: PathBuf,
    pub program: String,
    pub args: Vec<String>,
    pub timeout_ms: Option<u64>,
    pub max_output_bytes: Option<usize>,
    pub network_enabled: bool,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct ExecRequest {
    pub owner: RunnerOwner,
    pub session_key: Option<String>,
    pub workspace_root: PathBuf,
    pub cmd: String,
    pub workdir: Option<String>,
    pub shell: String,
    pub login: bool,
    pub tty: bool,
    pub timeout_ms: Option<u64>,
    pub yield_time_ms: Option<u64>,
    pub max_output_tokens: Option<usize>,
    pub max_output_bytes: Option<usize>,
    pub network_enabled: bool,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct InputRequest {
    pub owner: RunnerOwner,
    pub session_key: Option<String>,
    pub session_id: u64,
    pub chars: String,
    pub yield_time_ms: Option<u64>,
    pub max_output_tokens: Option<usize>,
    pub timeout_ms: Option<u64>,
    pub max_output_bytes: Option<usize>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct CancelSessionRequest {
    pub owner: RunnerOwner,
    pub session_key: Option<String>,
    pub session_id: u64,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct RunnerSessionStatus {
    pub session_id: u64,
    pub owner: RunnerOwner,
    pub session_key: Option<String>,
    pub state: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub wall_time_seconds: f64,
}

#[derive(
    Clone, Debug, Default, serde::Deserialize, serde::Serialize, schemars::JsonSchema, ToSchema,
)]
#[serde(deny_unknown_fields)]
pub struct CommandOutput {
    pub wall_time_seconds: f64,
    pub output: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub truncated: bool,
    pub timed_out: bool,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
}

#[derive(
    Clone, Debug, Default, serde::Deserialize, serde::Serialize, schemars::JsonSchema, ToSchema,
)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct ShellToolOutput {
    pub wall_time_seconds: f64,
    pub output: String,
    pub stdout: String,
    pub stderr: String,
    pub output_is_combined: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_token_count: Option<usize>,
    pub truncated: bool,
    pub has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub timed_out: bool,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, ToSchema)]
pub struct RunnerJob {
    pub job_id: String,
    pub kind: String,
    pub payload: Value,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, ToSchema)]
pub struct RunnerJobResult {
    pub job_id: String,
    pub ok: bool,
    pub result: Option<Value>,
    pub error: Option<String>,
}
