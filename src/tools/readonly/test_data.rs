use std::path::Path;

use rmcp::model::CallToolResult;

use crate::tools::{
    common::{mcp_error, structured_text_result},
    params::{GrepParams, ReadParams, StatPathParams},
    readonly::{
        data::{read_output, read_output_text, stat_path_output},
        search::search_files_output,
    },
};

pub(crate) fn read_file_result(
    root: &Path,
    params: &ReadParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let output = read_output(root, params).map_err(mcp_error)?;
    structured_text_result(read_output_text(&output), &output)
}

pub(crate) async fn search_files_result(
    root: &Path,
    params: &GrepParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let output = search_files_output(root, params).await.map_err(mcp_error)?;
    let text = output
        .matches
        .iter()
        .map(|entry| format!("{}:{}:{}", entry.path, entry.line_number, entry.line))
        .collect::<Vec<_>>()
        .join("\n");
    structured_text_result(text, &output)
}

pub(crate) fn stat_path_result(
    root: &Path,
    params: &StatPathParams,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let output = stat_path_output(root, params).map_err(mcp_error)?;
    let text = format!(
        "{}\nkind={}\nsize={}\nreadonly={}",
        output.path, output.kind, output.size, output.readonly
    );
    structured_text_result(text, &output)
}
