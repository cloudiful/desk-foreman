use schemars::JsonSchema;
use serde::Deserialize;
use utoipa::ToSchema;
use validator::Validate;

use crate::api::validation::{
    ReadFileRangeValidation, validate_non_blank, validate_read_file_params,
};

#[derive(Debug, Deserialize, JsonSchema, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct EmptyParams {}

#[derive(Debug, Deserialize, JsonSchema, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct ShellParams {
    #[schemars(description = "Shell command to execute inside the workspace.", example = &"pwd")]
    #[validate(custom(function = "validate_non_blank"))]
    pub command: String,
    #[serde(default)]
    #[schemars(description = "Workspace-relative directory to run the command from. Defaults to the workspace root when omitted.", example = &"src")]
    pub workdir: Option<String>,
    #[serde(default)]
    #[schemars(description = "Optional timeout in milliseconds.", example = 120000)]
    #[validate(range(min = 1, max = 600_000))]
    pub timeout: Option<u64>,
    #[serde(default)]
    #[schemars(
        description = "Approximate output token budget for the returned chunk.",
        example = 4000
    )]
    #[validate(range(min = 1))]
    pub max_output_tokens: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct WriteStdinParams {
    #[schemars(
        description = "Identifier of the existing shell session to write to or poll.",
        example = 1
    )]
    #[validate(range(min = 1))]
    pub session_id: u64,
    #[serde(default)]
    #[schemars(description = "Characters to write to the session. Omit or pass an empty string to poll for more output without writing.", example = &"help\n")]
    pub chars: String,
    #[serde(default)]
    #[schemars(
        description = "Milliseconds to wait before returning more output from the session.",
        example = 1000
    )]
    #[validate(range(min = 0))]
    pub yield_time_ms: Option<u64>,
    #[serde(default)]
    #[schemars(
        description = "Approximate output token budget for the returned chunk.",
        example = 4000
    )]
    #[validate(range(min = 1))]
    pub max_output_tokens: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct CancelSessionParams {
    #[schemars(
        description = "Identifier of the active shell session to terminate.",
        example = 1
    )]
    #[validate(range(min = 1))]
    pub session_id: u64,
}

#[derive(Debug, Deserialize, JsonSchema, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct ApplyPatchParams {
    #[serde(rename = "patchText")]
    #[schemars(rename = "patchText", description = "Codex patch DSL text to apply inside the workspace.", example = &"*** Begin Patch\n*** Add File: example.txt\n+content\n*** End Patch\n")]
    #[validate(custom(function = "validate_non_blank"))]
    pub patch_text: String,
}

#[derive(Debug, Deserialize, JsonSchema, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct EditParams {
    #[schemars(
        description = "Workspace-relative file path to edit.",
        example = "src/lib.rs"
    )]
    #[validate(custom(function = "validate_non_blank"))]
    pub path: String,
    #[schemars(
        description = "Exact text to replace. It must occur exactly once unless replace_all is true."
    )]
    pub old_text: String,
    #[schemars(description = "Replacement text. It must differ from old_text.")]
    pub new_text: String,
    #[serde(default)]
    #[schemars(
        description = "Replace every exact occurrence instead of requiring a unique match."
    )]
    pub replace_all: bool,
}

#[derive(Debug, Deserialize, JsonSchema, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
#[validate(schema(function = "validate_read_file_params"))]
pub struct ReadParams {
    #[serde(rename = "filePath")]
    #[schemars(rename = "filePath", description = "Workspace-relative or workspace-contained absolute path to read.", example = &"README.md")]
    #[validate(custom(function = "validate_non_blank"))]
    pub file_path: String,
    #[serde(default)]
    #[schemars(
        description = "First 1-based line number to include. Defaults to 1.",
        example = 10
    )]
    #[validate(range(min = 1))]
    pub offset: Option<usize>,
    #[serde(default)]
    #[schemars(
        description = "Maximum number of lines to return. Defaults to 2000.",
        example = 2000
    )]
    #[validate(range(min = 1, max = 2000))]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct GrepParams {
    #[schemars(description = "Regular expression pattern to search for in file contents.", example = &"WorkspaceFileTools")]
    #[validate(custom(function = "validate_non_blank"))]
    pub pattern: String,
    #[serde(default = "default_dot")]
    #[schemars(description = "Workspace-relative directory to search under. Defaults to `.`.", default = "default_dot", example = &".")]
    #[validate(custom(function = "validate_non_blank"))]
    pub path: String,
    #[serde(default)]
    #[schemars(description = "Optional ripgrep file include glob.", example = &"*.rs")]
    pub include: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct GlobParams {
    #[schemars(description = "Glob pattern used to find workspace files.", example = &"**/*.rs")]
    #[validate(custom(function = "validate_non_blank"))]
    pub pattern: String,
    #[serde(default = "default_dot")]
    #[schemars(description = "Workspace-relative directory to search under. Defaults to `.`.", default = "default_dot", example = &".")]
    #[validate(custom(function = "validate_non_blank"))]
    pub path: String,
}

#[derive(Debug, Deserialize, JsonSchema, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct StatPathParams {
    #[schemars(description = "Workspace-relative file or directory path to inspect.", example = &"README.md")]
    #[validate(custom(function = "validate_non_blank"))]
    pub path: String,
}

pub(super) fn default_dot() -> String {
    ".".to_string()
}

impl ReadFileRangeValidation for ReadParams {
    fn start_line(&self) -> Option<usize> {
        self.offset
    }

    fn end_line(&self) -> Option<usize> {
        self.offset
            .zip(self.limit)
            .and_then(|(offset, limit)| offset.checked_add(limit.saturating_sub(1)))
    }
}

impl ReadFileRangeValidation for &ReadParams {
    fn start_line(&self) -> Option<usize> {
        self.offset
    }

    fn end_line(&self) -> Option<usize> {
        self.offset
            .zip(self.limit)
            .and_then(|(offset, limit)| offset.checked_add(limit.saturating_sub(1)))
    }
}
