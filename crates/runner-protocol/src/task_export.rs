//! Strict protocol and shared validation for selected native-task exports.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
    NativeTaskRuntimeIdentity, RunnerOwner, SeedBundleError, SourceSeedSnapshotFile,
    validate_seed_bundle,
};

mod output;
pub use output::{native_task_export_output_manifest_sha256, validate_native_task_export_content};

#[cfg(test)]
#[path = "task_export/tests.rs"]
mod tests;

pub const MAX_TASK_EXPORT_PATHS: usize = 4096;
pub const MAX_TASK_EXPORT_PATH_BYTES: usize = 512;
pub const MAX_TASK_EXPORT_UPLOAD_BODY_BYTES: usize = 16 * 1024 * 1024;
pub const TASK_EXPORT_SELECTION_PREFIX: &[u8] = b"df-native-task-export-selection-v1\0";
pub const TASK_EXPORT_OUTPUT_PREFIX: &[u8] = b"df-native-task-export-v1\0";

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportNativeTaskRequest {
    pub owner: RunnerOwner,
    pub task: NativeTaskRuntimeIdentity,
    pub stop_job_id: Uuid,
    pub selection_sha256: String,
    pub paths: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeTaskExportContent {
    pub task_id: Uuid,
    pub stop_job_id: Uuid,
    pub selection_sha256: String,
    pub output_manifest_sha256: String,
    pub files: Vec<SourceSeedSnapshotFile>,
    pub missing_paths: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportNativeTaskUpload {
    pub request: ExportNativeTaskRequest,
    pub content: NativeTaskExportContent,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportNativeTaskResult {
    pub task_id: Uuid,
    pub stop_job_id: Uuid,
    pub selection_sha256: String,
    pub output_manifest_sha256: String,
    pub file_count: u32,
    pub missing_count: u32,
    pub total_bytes: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeTaskExportSelection {
    pub paths: Vec<String>,
    pub selection_sha256: String,
    pub file_count: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedNativeTaskExport {
    pub content: NativeTaskExportContent,
    pub file_count: u32,
    pub missing_count: u32,
    pub total_bytes: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeTaskExportValidationError {
    EmptySelection,
    TooManyPaths,
    SelectionBounds(SeedBundleError),
    OutputBounds(SeedBundleError),
    IdentityMismatch,
    SelectionDigestMismatch,
    InvalidMissingPaths(SeedBundleError),
    OutputSetMismatch,
    OutputManifestMismatch,
}

impl NativeTaskExportValidationError {
    pub fn is_too_large(self) -> bool {
        matches!(self, Self::TooManyPaths)
            || matches!(
                self,
                Self::SelectionBounds(
                    SeedBundleError::TooManyEntries
                        | SeedBundleError::ContentTooLarge
                        | SeedBundleError::TotalTooLarge
                ) | Self::OutputBounds(
                    SeedBundleError::TooManyEntries
                        | SeedBundleError::ContentTooLarge
                        | SeedBundleError::TotalTooLarge
                ) | Self::InvalidMissingPaths(
                    SeedBundleError::TooManyEntries
                        | SeedBundleError::ContentTooLarge
                        | SeedBundleError::TotalTooLarge
                )
            )
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::EmptySelection => "empty_selection",
            Self::TooManyPaths => "too_many_paths",
            Self::SelectionBounds(error) => error.code(),
            Self::OutputBounds(error) | Self::InvalidMissingPaths(error) => error.code(),
            Self::IdentityMismatch => "identity_mismatch",
            Self::SelectionDigestMismatch => "selection_digest_mismatch",
            Self::OutputSetMismatch => "output_set_mismatch",
            Self::OutputManifestMismatch => "output_manifest_mismatch",
        }
    }

    pub fn is_conflict(self) -> bool {
        matches!(
            self,
            Self::IdentityMismatch
                | Self::SelectionDigestMismatch
                | Self::OutputSetMismatch
                | Self::OutputManifestMismatch
        )
    }
}

pub fn validate_native_task_export_selection(
    paths: &[String],
) -> Result<NativeTaskExportSelection, NativeTaskExportValidationError> {
    if paths.is_empty() {
        return Err(NativeTaskExportValidationError::EmptySelection);
    }
    if paths.len() > MAX_TASK_EXPORT_PATHS {
        return Err(NativeTaskExportValidationError::TooManyPaths);
    }
    let path_files: Vec<_> = paths
        .iter()
        .map(|path| SourceSeedSnapshotFile::new(path.clone(), ""))
        .collect();
    let bundle = validate_seed_bundle(&path_files)
        .map_err(NativeTaskExportValidationError::SelectionBounds)?;
    if bundle
        .files
        .iter()
        .any(|file| file.path.len() > MAX_TASK_EXPORT_PATH_BYTES)
    {
        return Err(NativeTaskExportValidationError::SelectionBounds(
            SeedBundleError::PathTooLong,
        ));
    }
    let paths = bundle
        .files
        .into_iter()
        .map(|file| file.path)
        .collect::<Vec<_>>();
    let selection_sha256 = selection_manifest_sha256(&paths);
    Ok(NativeTaskExportSelection {
        file_count: u32::try_from(paths.len()).unwrap_or_default(),
        paths,
        selection_sha256,
    })
}

pub fn selection_manifest_sha256(paths: &[String]) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(TASK_EXPORT_SELECTION_PREFIX);
    for path in paths {
        hasher.update((path.len() as u64).to_be_bytes());
        hasher.update(path.as_bytes());
    }
    lower_hex(&hasher.finalize())
}

fn lower_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}
