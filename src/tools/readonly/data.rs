use std::path::Path;

use crate::{
    AppState,
    actor::ActorContext,
    pathing::{resolve_workspace_path, workspace_relative_display},
    tools::{
        common::{ToolError, run_command_in_runner, tool_internal, tool_invalid_input},
        params::{GlobParams, ReadParams, StatPathParams},
        readonly::types::{DirectoryEntry, GlobOutput, ReadOutput, StatPathOutput},
    },
};
use desk_foreman_workspace_sdk::{
    ListDirectoryPageParams as WorkspaceListDirectoryPageParams, PathKind,
    ReadFilePageParams as WorkspaceReadFilePageParams, StatPathOutput as WorkspaceStatPathOutput,
    StatPathParams as WorkspaceStatPathParams, WorkspaceFileTools, WorkspaceSdkError,
};

const MAX_READ_BYTES: usize = 50 * 1024;
const MAX_READ_LINE_BYTES: usize = 16 * 1024;
const PROTECTED_RG_GLOBS: [&str; 10] = [
    "!.env",
    "!.env.*",
    "!**/.env",
    "!**/.env.*",
    "!**/credentials/**",
    "!**/.ssh/**",
    "!**/.aws/**",
    "!**/*.pem",
    "!**/*.key",
    "!**/id_rsa*",
];

pub(crate) fn read_output(root: &Path, params: &ReadParams) -> Result<ReadOutput, ToolError> {
    let tools = workspace_tools(root)?;
    let workspace_root = tools.workspace_root().to_path_buf();
    let absolute = resolve_input_path(&workspace_root, &params.file_path)?;
    let relative = workspace_relative_display(&workspace_root, &absolute);
    let metadata = tools
        .stat_path(&WorkspaceStatPathParams {
            path: relative.clone(),
        })
        .map_err(map_readonly_sdk_error)?;
    let offset = params.offset.unwrap_or(1);
    let limit = params.limit.unwrap_or(2000);
    if matches!(metadata.kind, PathKind::Dir) {
        let page = tools
            .list_directory_page(&WorkspaceListDirectoryPageParams {
                path: relative.clone(),
                offset: offset.saturating_sub(1),
                limit,
            })
            .map_err(map_readonly_sdk_error)?;
        let entries = page
            .entries
            .into_iter()
            .map(|entry| DirectoryEntry {
                path: entry.path,
                kind: path_kind_label(entry.kind).to_string(),
            })
            .collect::<Vec<_>>();
        return Ok(ReadOutput {
            path: page.path,
            kind: "directory".to_string(),
            content: None,
            entries,
            offset,
            total: page.total_entries,
            truncated: page.truncated,
        });
    }

    let page = tools
        .read_file_page(&WorkspaceReadFilePageParams {
            path: relative,
            start_line: offset,
            max_lines: limit,
            max_bytes: MAX_READ_BYTES,
        })
        .map_err(map_readonly_sdk_error)?;
    let (content, line_truncated) = limit_line_lengths(&page.content, MAX_READ_LINE_BYTES);
    Ok(ReadOutput {
        path: page.path,
        kind: "file".to_string(),
        content: Some(content),
        entries: Vec::new(),
        offset: page.start_line,
        total: page.total_lines,
        truncated: page.truncated || line_truncated,
    })
}

pub(crate) fn read_output_text(output: &ReadOutput) -> String {
    match output.kind.as_str() {
        "directory" => output
            .entries
            .iter()
            .map(|entry| {
                if entry.kind == "dir" {
                    format!("{}/", entry.path)
                } else {
                    entry.path.clone()
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => number_lines(output.content.as_deref().unwrap_or_default(), output.offset),
    }
}

pub(crate) async fn glob_output_in_runner(
    state: &AppState,
    actor: &ActorContext,
    params: &GlobParams,
) -> Result<GlobOutput, ToolError> {
    let search_root =
        resolve_workspace_path(&actor.workspace_root, &params.path).map_err(tool_invalid_input)?;
    let mut args = vec![
        "--files".to_string(),
        "--color".to_string(),
        "never".to_string(),
    ];
    args.extend(protected_rg_args_owned());
    args.extend(["--glob".to_string(), params.pattern.clone()]);
    let output = run_command_in_runner(state, actor, search_root.clone(), "rg", args).await?;
    const LIMIT: usize = 100;
    let mut matches = output
        .output
        .lines()
        .map(|path| workspace_relative_display(&actor.workspace_root, &search_root.join(path)))
        .take(LIMIT)
        .collect::<Vec<_>>();
    matches.sort();
    Ok(GlobOutput {
        path: workspace_relative_display(&actor.workspace_root, &search_root),
        pattern: params.pattern.clone(),
        truncated: output.truncated || output.output.lines().nth(LIMIT).is_some(),
        matches,
    })
}

pub(crate) fn stat_path_output(
    root: &Path,
    params: &StatPathParams,
) -> Result<StatPathOutput, ToolError> {
    let tools = workspace_tools(root)?;
    let output = tools
        .stat_path(&WorkspaceStatPathParams {
            path: params.path.clone(),
        })
        .map_err(map_readonly_sdk_error)?;
    Ok(convert_stat_path_output(output))
}

fn workspace_tools(root: &Path) -> Result<WorkspaceFileTools, ToolError> {
    WorkspaceFileTools::new(root).map_err(map_readonly_sdk_error)
}

fn map_readonly_sdk_error(error: WorkspaceSdkError) -> ToolError {
    match error {
        WorkspaceSdkError::InvalidInput(message) => ToolError::InvalidInput(message),
        other => ToolError::Internal(other.into()),
    }
}

fn resolve_input_path(root: &Path, input: &str) -> Result<std::path::PathBuf, ToolError> {
    if std::path::Path::new(input).is_absolute() {
        let root = root.canonicalize().map_err(tool_internal)?;
        let absolute = std::path::Path::new(input)
            .canonicalize()
            .map_err(tool_invalid_input)?;
        if !absolute.starts_with(&root) {
            return Err(tool_invalid_input(anyhow::anyhow!(
                "path escapes workspace root"
            )));
        }
        Ok(absolute)
    } else {
        resolve_workspace_path(root, input).map_err(tool_invalid_input)
    }
}

fn convert_stat_path_output(output: WorkspaceStatPathOutput) -> StatPathOutput {
    StatPathOutput {
        path: output.path,
        kind: path_kind_label(output.kind).to_string(),
        size: output.size,
        readonly: output.readonly,
    }
}

fn path_kind_label(kind: PathKind) -> &'static str {
    kind.as_str()
}

pub(crate) fn protected_rg_args() -> impl Iterator<Item = &'static str> {
    PROTECTED_RG_GLOBS
        .iter()
        .copied()
        .flat_map(|pattern| ["--glob", pattern])
}

pub(crate) fn protected_rg_args_owned() -> Vec<String> {
    protected_rg_args().map(str::to_string).collect()
}

fn number_lines(content: &str, start_line: usize) -> String {
    let mut numbered = content
        .lines()
        .enumerate()
        .map(|(offset, line)| format!("{:>6}\t{}", start_line + offset, line))
        .collect::<Vec<_>>()
        .join("\n");
    if content.ends_with(['\n', '\r']) && !numbered.is_empty() {
        numbered.push('\n');
    }
    numbered
}

fn limit_line_lengths(content: &str, max_line_bytes: usize) -> (String, bool) {
    let mut output = String::with_capacity(content.len().min(MAX_READ_BYTES));
    let mut truncated = false;
    for segment in content.split_inclusive('\n') {
        let (line, newline) = segment
            .strip_suffix('\n')
            .map_or((segment, ""), |line| (line, "\n"));
        let mut end = line.len().min(max_line_bytes);
        while end > 0 && !line.is_char_boundary(end) {
            end -= 1;
        }
        if end < line.len() {
            truncated = true;
        }
        output.push_str(&line[..end]);
        output.push_str(newline);
    }
    (output, truncated)
}
