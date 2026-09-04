use std::path::{Path, PathBuf};

use serde_json::Value;

#[cfg(test)]
use std::process::Stdio;
#[cfg(test)]
use tokio::process::Command;

use crate::{
    AppState,
    actor::ActorContext,
    pathing::{resolve_workspace_path, workspace_relative_display},
    tools::{
        common::{ToolError, run_command_in_runner, tool_internal, tool_invalid_input},
        params::GrepParams,
        readonly::{
            data::protected_rg_args_owned,
            types::{GrepOutput, SearchMatch},
        },
    },
};

#[cfg(test)]
use crate::tools::readonly::data::protected_rg_args;

const MAX_SEARCH_MATCHES: usize = 100;

struct GrepTarget {
    workdir: PathBuf,
    join_base: PathBuf,
    display_target: PathBuf,
    file_arg: Option<String>,
}

fn resolve_grep_target(workspace_root: &Path, input: &str) -> Result<GrepTarget, ToolError> {
    let target = resolve_workspace_path(workspace_root, input).map_err(tool_invalid_input)?;
    match std::fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.is_dir() => Ok(GrepTarget {
            workdir: target.clone(),
            join_base: target.clone(),
            display_target: target,
            file_arg: None,
        }),
        Ok(metadata) if metadata.is_file() => {
            let parent = target
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| tool_invalid_input(anyhow::anyhow!("invalid grep path")))?;
            let file_name = target
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| tool_invalid_input(anyhow::anyhow!("invalid grep path")))?
                .to_string();
            Ok(GrepTarget {
                workdir: parent.clone(),
                join_base: parent,
                display_target: target,
                file_arg: Some(file_name),
            })
        }
        Ok(_) => Err(tool_invalid_input(anyhow::anyhow!(
            "grep path must be a file or directory"
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(ToolError::NotFound(format!("grep path not found: {input}")))
        }
        Err(error) => Err(tool_internal(error)),
    }
}

fn grep_base_args(params: &GrepParams) -> Vec<String> {
    let mut args = vec![
        "-n".to_string(),
        "--json".to_string(),
        "--color".to_string(),
        "never".to_string(),
    ];
    args.extend(protected_rg_args_owned());
    if let Some(include) = &params.include {
        args.extend(["--glob".to_string(), include.clone()]);
    }
    args.push(params.pattern.clone());
    args
}

#[cfg(test)]
pub(crate) async fn search_files_output(
    root: &Path,
    params: &GrepParams,
) -> Result<GrepOutput, ToolError> {
    let target = resolve_grep_target(root, &params.path)?;
    let mut command = Command::new("rg");
    command
        .arg("-n")
        .arg("--json")
        .arg("--color")
        .arg("never")
        .args(protected_rg_args())
        .args(
            params
                .include
                .as_ref()
                .map(|include| ["--glob", include])
                .into_iter()
                .flatten(),
        )
        .arg(&params.pattern);
    if let Some(file_arg) = &target.file_arg {
        command.arg(file_arg);
    }
    let output = command
        .current_dir(&target.workdir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(tool_internal)?;
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(tool_internal(anyhow::anyhow!(
            "rg failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    parse_search_output(
        root,
        &target.join_base,
        &target.display_target,
        &params.pattern,
        String::from_utf8_lossy(&output.stdout).lines(),
    )
}

pub(crate) async fn search_files_output_in_runner(
    state: &AppState,
    actor: &ActorContext,
    params: &GrepParams,
) -> Result<GrepOutput, ToolError> {
    let target = resolve_grep_target(&actor.workspace_root, &params.path)?;
    let mut args = grep_base_args(params);
    if let Some(file_arg) = &target.file_arg {
        args.push(file_arg.clone());
    }
    let output = run_command_in_runner(state, actor, target.workdir.clone(), "rg", args).await?;
    match output.exit_code {
        Some(0) | Some(1) => {}
        Some(code) => {
            let stderr = output.stderr.trim();
            let message = if stderr.is_empty() {
                format!("rg failed with exit code {code}")
            } else {
                format!("rg failed with exit code {code}: {stderr}")
            };
            return Err(tool_internal(anyhow::anyhow!(message)));
        }
        None => {
            if output.timed_out {
                return Err(tool_internal(anyhow::anyhow!("rg timed out")));
            }
            let stderr = output.stderr.trim();
            let message = if stderr.is_empty() {
                "rg failed: no exit code".to_string()
            } else {
                format!("rg failed: {stderr}")
            };
            return Err(tool_internal(anyhow::anyhow!(message)));
        }
    }
    parse_search_output(
        &actor.workspace_root,
        &target.join_base,
        &target.display_target,
        &params.pattern,
        output.stdout.lines(),
    )
}

fn parse_search_output<'a, I>(
    workspace_root: &Path,
    join_base: &Path,
    display_target: &Path,
    pattern: &str,
    lines: I,
) -> Result<GrepOutput, ToolError>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut matches = Vec::new();
    let mut truncated = false;
    for raw_line in lines {
        let event: Value = serde_json::from_str(raw_line).map_err(tool_internal)?;
        if event.get("type").and_then(Value::as_str) != Some("match") {
            continue;
        }
        let data = event
            .get("data")
            .ok_or_else(|| tool_internal(anyhow::anyhow!("rg match event missing data")))?;
        if matches.len() >= MAX_SEARCH_MATCHES {
            truncated = true;
            break;
        }
        matches.push(SearchMatch {
            path: workspace_relative_display(workspace_root, &join_base.join(relative_path(data))),
            line_number: line_number(data)?,
            line: line_text(data),
        });
    }
    Ok(GrepOutput {
        path: workspace_relative_display(workspace_root, display_target),
        pattern: pattern.to_string(),
        matches,
        truncated,
    })
}

fn relative_path(data: &Value) -> &str {
    data.get("path")
        .and_then(|value| value.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn line_number(data: &Value) -> Result<usize, ToolError> {
    data.get("line_number")
        .and_then(Value::as_u64)
        .map(|number| number as usize)
        .ok_or_else(|| tool_internal(anyhow::anyhow!("rg match event missing line_number")))
}

fn line_text(data: &Value) -> String {
    data.get("lines")
        .and_then(|value| value.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_end_matches('\n')
        .to_string()
}
