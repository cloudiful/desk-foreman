use std::path::Path;

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

#[cfg(test)]
pub(crate) async fn search_files_output(
    root: &Path,
    params: &GrepParams,
) -> Result<GrepOutput, ToolError> {
    let search_root = resolve_workspace_path(root, &params.path).map_err(tool_invalid_input)?;
    let output = Command::new("rg")
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
        .arg(&params.pattern)
        .current_dir(&search_root)
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
        &search_root,
        &params.pattern,
        String::from_utf8_lossy(&output.stdout).lines(),
    )
}

pub(crate) async fn search_files_output_in_runner(
    state: &AppState,
    actor: &ActorContext,
    params: &GrepParams,
) -> Result<GrepOutput, ToolError> {
    let search_root =
        resolve_workspace_path(&actor.workspace_root, &params.path).map_err(tool_invalid_input)?;
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
    let output = run_command_in_runner(state, actor, search_root.clone(), "rg", args).await?;
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
        &search_root,
        &params.pattern,
        output.stdout.lines(),
    )
}

fn parse_search_output<'a, I>(
    workspace_root: &Path,
    search_root: &Path,
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
            path: workspace_relative_display(
                workspace_root,
                &search_root.join(relative_path(data)),
            ),
            line_number: line_number(data)?,
            line: line_text(data),
        });
    }
    Ok(GrepOutput {
        path: workspace_relative_display(workspace_root, search_root),
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
