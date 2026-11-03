use serde::{Deserialize, Serialize};

/// Closed set of patch failure codes. Every code carries one fixed diagnostic
/// message so callers cannot inject raw IO, expected/found text or file content
/// into a patch result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchCode {
    PatchArgInvalid,
    PatchTooLarge,
    PatchBoundaryInvalid,
    PatchHunkInvalid,
    PatchPathInvalid,
    PatchPathProtected,
    PatchTargetMissing,
    PatchTargetExists,
    PatchTargetNotRegular,
    PatchNotUtf8Text,
    PatchContextNotFound,
    PatchContextAmbiguous,
    PatchMoveConflict,
    PatchDuplicateTarget,
    PatchTargetChanged,
    PatchSourceChanged,
    PatchWriteFailed,
    PatchFenceFailed,
}

impl PatchCode {
    pub fn message(self) -> &'static str {
        match self {
            Self::PatchArgInvalid => "patch argument is invalid",
            Self::PatchTooLarge => "patch exceeds the allowed size",
            Self::PatchBoundaryInvalid => "patch envelope is missing or malformed",
            Self::PatchHunkInvalid => "patch hunk is malformed",
            Self::PatchPathInvalid => "patch path is not a valid workspace-relative path",
            Self::PatchPathProtected => "patch path is protected by workspace policy",
            Self::PatchTargetMissing => "patch target is missing or unreadable",
            Self::PatchTargetExists => "patch target already exists",
            Self::PatchTargetNotRegular => "patch target is not a regular file",
            Self::PatchNotUtf8Text => "patch target is not valid UTF-8 text",
            Self::PatchContextNotFound => "patch context was not found",
            Self::PatchContextAmbiguous => "patch context is ambiguous",
            Self::PatchMoveConflict => "patch move conflicts with its destination",
            Self::PatchDuplicateTarget => "patch contains conflicting operations for the same path",
            Self::PatchTargetChanged => "patch target changed during apply",
            Self::PatchSourceChanged => "patch move source changed during apply",
            Self::PatchWriteFailed => "patch write failed",
            Self::PatchFenceFailed => "workspace write fence failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchStage {
    Params,
    Parse,
    Plan,
    Commit,
    Fence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchEffect {
    Changed,
    Unchanged,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchWrites {
    None,
    All,
    Partial,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchDiagnostic {
    pub code: PatchCode,
    pub stage: PatchStage,
    pub path: Option<String>,
    pub operation: Option<ApplyPatchOperation>,
    pub hunk: Option<u32>,
    message: String,
}

impl PatchDiagnostic {
    pub fn new(code: PatchCode, stage: PatchStage) -> Self {
        Self {
            code,
            stage,
            path: None,
            operation: None,
            hunk: None,
            message: code.message().to_string(),
        }
    }

    pub fn with_path(mut self, path: Option<String>) -> Self {
        self.path = path;
        self
    }

    pub fn with_operation(mut self, operation: Option<ApplyPatchOperation>) -> Self {
        self.operation = operation;
        self
    }

    pub fn with_hunk(mut self, hunk: Option<u32>) -> Self {
        self.hunk = hunk;
        self
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchChangeError {
    pub code: PatchCode,
    message: String,
}

impl PatchChangeError {
    pub fn new(code: PatchCode) -> Self {
        Self {
            code,
            message: code.message().to_string(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchChange {
    pub operation: ApplyPatchOperation,
    pub path: String,
    pub destination: Option<String>,
    pub status: ApplyPatchStatus,
    pub effect: PatchEffect,
    pub error: Option<PatchChangeError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchApplied {
    pub changes: Vec<PatchChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchRejected {
    pub diagnostic: PatchDiagnostic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchIncomplete {
    pub writes: PatchWrites,
    pub changes: Vec<PatchChange>,
    pub diagnostic: PatchDiagnostic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PatchResult {
    Applied(PatchApplied),
    Rejected(PatchRejected),
    Incomplete(PatchIncomplete),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyPatchOperation {
    Add,
    Delete,
    Update,
    Move,
}

impl ApplyPatchOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Delete => "delete",
            Self::Update => "update",
            Self::Move => "move",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyPatchStatus {
    Applied,
    Failed,
    Skipped,
}

impl ApplyPatchStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadFilePageParams {
    pub path: String,
    pub start_line: usize,
    pub max_lines: usize,
    pub max_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadFilePageOutput {
    pub path: String,
    pub content: String,
    pub start_line: usize,
    pub end_line: usize,
    pub total_lines: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathKind {
    File,
    Dir,
}

impl PathKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Dir => "dir",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryEntry {
    pub path: String,
    pub kind: PathKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListDirectoryPageParams {
    pub path: String,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListDirectoryPageOutput {
    pub path: String,
    pub entries: Vec<DirectoryEntry>,
    pub offset: usize,
    pub total_entries: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalkWorkspacePageParams {
    pub path: String,
    pub max_entries: usize,
    pub max_depth: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalkWorkspacePageOutput {
    pub path: String,
    pub entries: Vec<DirectoryEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatPathParams {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatPathOutput {
    pub path: String,
    pub kind: PathKind,
    pub size: u64,
    pub readonly: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFingerprint {
    pub path: String,
    pub kind: PathKind,
    pub size: u64,
    pub sha256: Option<String>,
}
