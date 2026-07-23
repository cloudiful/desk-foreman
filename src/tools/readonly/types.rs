use schemars::JsonSchema;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadOutput {
    pub path: String,
    pub kind: String,
    pub content: Option<String>,
    pub entries: Vec<DirectoryEntry>,
    pub offset: usize,
    pub total: usize,
    pub truncated: bool,
}

#[derive(Debug, Serialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct DirectoryEntry {
    pub path: String,
    pub kind: String,
}

#[derive(Debug, Serialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct SearchMatch {
    pub path: String,
    pub line_number: usize,
    pub line: String,
}

#[derive(Debug, Serialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct GrepOutput {
    pub path: String,
    pub pattern: String,
    pub matches: Vec<SearchMatch>,
    pub truncated: bool,
}

#[derive(Debug, Serialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GlobOutput {
    pub path: String,
    pub pattern: String,
    pub matches: Vec<String>,
    pub truncated: bool,
}

#[derive(Debug, Serialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct StatPathOutput {
    pub path: String,
    pub kind: String,
    pub size: u64,
    pub readonly: bool,
}
