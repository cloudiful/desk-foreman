mod error;
mod fs_tools;
mod patch;
mod path;
mod types;

#[cfg(feature = "approval")]
pub use desk_foreman_approval::{
    ApprovalError, ApprovalFuture, ApprovalReviewer, OpenAiReviewer, OpenAiReviewerConfig,
    ReviewAction, ReviewDecision, ReviewDecisionKind, ReviewRequest, ReviewRisk,
};
pub use error::WorkspaceSdkError;
#[cfg(feature = "approval")]
pub use fs_tools::ReviewedWorkspaceFileTools;
pub use fs_tools::WorkspaceFileTools;
pub use patch::{Hunk, HunkLine, ParsedPatch, PatchOperation, parse_patch};
pub use path::{resolve_workspace_path, workspace_relative_display};
pub use types::{
    ApplyPatchChange, ApplyPatchOperation, ApplyPatchStatus, ApplyPatchSummary, DirectoryEntry,
    FileFingerprint, ListDirectoryPageOutput, ListDirectoryPageParams, PathKind,
    ReadFilePageOutput, ReadFilePageParams, StatPathOutput, StatPathParams,
    WalkWorkspacePageOutput, WalkWorkspacePageParams,
};
