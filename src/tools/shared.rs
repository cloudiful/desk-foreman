use std::time::Instant;

use desk_foreman_workspace_sdk::{
    ApplyPatchSummary, PatchOperation, WorkspaceFileTools, WorkspaceSdkError, parse_patch,
};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;
use utoipa::ToSchema;

use crate::{
    AppState,
    actor::ActorContext,
    tools::{
        common::{
            ToolError, ensure_scope, sha256_hex, spawn_tool_audit, tool_internal,
            validate_shell_binary, validate_shell_command, validate_workspace_path,
        },
        params::{
            ApplyPatchParams, GlobParams, GrepParams, ReadParams, ShellParams, WriteStdinParams,
        },
        readonly::{
            data::{glob_output_in_runner, read_output, stat_path_output},
            search::search_files_output_in_runner,
            types::{GlobOutput, GrepOutput, ReadOutput, StatPathOutput},
        },
    },
};
use runner_protocol::{CancelSessionRequest, ExecRequest, InputRequest, ShellToolOutput};

const DEFAULT_SHELL_TIMEOUT_MS: u64 = 120_000;

#[derive(Clone, Debug, Serialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct ApplyPatchOutput {
    pub summary: String,
    pub partial: bool,
    pub changes: Vec<ApplyPatchChangeOutput>,
}

#[derive(Clone, Debug, Serialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct ApplyPatchChangeOutput {
    pub operation: String,
    pub path: String,
    pub destination: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub added_lines: usize,
    pub deleted_lines: usize,
}

#[derive(Clone, Debug, Serialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct CancelSessionOutput {
    pub session_id: u64,
    pub state: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub wall_time_seconds: f64,
}

impl From<ApplyPatchSummary> for ApplyPatchOutput {
    fn from(summary: ApplyPatchSummary) -> Self {
        Self {
            summary: summary.summary,
            partial: summary.partial,
            changes: summary
                .changes
                .into_iter()
                .map(|change| ApplyPatchChangeOutput {
                    operation: change.operation.as_str().to_string(),
                    path: change.path,
                    destination: change.destination,
                    status: change.status.as_str().to_string(),
                    error: change.error,
                    added_lines: change.added_lines,
                    deleted_lines: change.deleted_lines,
                })
                .collect(),
        }
    }
}

fn workspace_tools(actor: &ActorContext) -> Result<WorkspaceFileTools, ToolError> {
    WorkspaceFileTools::new(&actor.workspace_root).map_err(|error| match error {
        WorkspaceSdkError::InvalidInput(message) => ToolError::InvalidInput(message),
        WorkspaceSdkError::PatchContextNotFound(message) => {
            ToolError::PatchContextNotFound(message)
        }
        other => ToolError::Internal(other.into()),
    })
}

pub async fn shell(
    state: &AppState,
    actor: &ActorContext,
    params: &ShellParams,
) -> Result<ShellToolOutput, ToolError> {
    let started = Instant::now();
    ensure_scope(state, actor, crate::policy::WORKSPACE_SHELL)?;
    actor.ensure_write_access().map_err(ToolError::Forbidden)?;
    if let Some(limit) = actor.policy.limits.max_sessions {
        let active = state
            .runner
            .list_sessions()
            .await
            .map_err(tool_internal)?
            .into_iter()
            .filter(|session| session.owner == actor.runner_owner())
            .count();
        if active >= limit {
            return Err(ToolError::Forbidden("session limit reached".to_string()));
        }
    }
    validate_shell_command(&params.command)?;
    if let Some(workdir) = &params.workdir {
        validate_workspace_path(workdir)?;
    }
    let shell = actor
        .application
        .as_ref()
        .and_then(|application| application.default_shell.clone())
        .unwrap_or_else(|| state.config.default_shell.clone());
    validate_shell_binary(&shell)?;
    let output = state
        .runner
        .exec_shell(ExecRequest {
            owner: actor.runner_owner(),
            session_key: Some(actor.principal_id.clone()),
            workspace_root: actor.workspace_root.clone(),
            cmd: params.command.clone(),
            workdir: params.workdir.clone(),
            shell,
            login: false,
            tty: false,
            timeout_ms: Some(
                params
                    .timeout
                    .unwrap_or(DEFAULT_SHELL_TIMEOUT_MS)
                    .min(actor.policy.limits.max_timeout_ms.unwrap_or(u64::MAX)),
            ),
            yield_time_ms: Some(1000),
            max_output_tokens: params.max_output_tokens,
            max_output_bytes: actor.policy.limits.max_output_bytes,
            network_enabled: actor.policy.limits.network_enabled,
        })
        .await
        .map_err(classify_shell_error)?;
    spawn_tool_audit(
        state,
        actor,
        "tool.shell",
        json!({
            "command_sha256": sha256_hex(&params.command),
            "status": if output.exit_code == Some(0) { "success" } else { "failed" },
            "exit_code": output.exit_code,
            "stdout_bytes": output.stdout_bytes,
            "stderr_bytes": output.stderr_bytes,
            "truncated": output.truncated,
            "timed_out": output.timed_out,
            "duration_ms": started.elapsed().as_millis(),
        }),
    );
    Ok(output)
}

pub async fn write_stdin(
    state: &AppState,
    actor: &ActorContext,
    params: &WriteStdinParams,
) -> Result<ShellToolOutput, ToolError> {
    let started = Instant::now();
    ensure_scope(state, actor, crate::policy::WORKSPACE_SHELL)?;
    if !params.chars.is_empty() {
        actor.ensure_write_access().map_err(ToolError::Forbidden)?;
    }
    let output = state
        .runner
        .write_stdin(InputRequest {
            owner: actor.runner_owner(),
            session_key: Some(actor.principal_id.clone()),
            session_id: params.session_id,
            chars: params.chars.clone(),
            yield_time_ms: params.yield_time_ms,
            max_output_tokens: params.max_output_tokens,
            timeout_ms: Some(
                actor
                    .policy
                    .limits
                    .max_timeout_ms
                    .unwrap_or(120_000)
                    .min(120_000),
            ),
            max_output_bytes: Some(actor.policy.limits.max_output_bytes.unwrap_or(256 * 1024)),
        })
        .await
        .map_err(classify_shell_error)?;
    spawn_tool_audit(
        state,
        actor,
        "tool.write_stdin",
        json!({
            "session_id": params.session_id,
            "input_sha256": sha256_hex(&params.chars),
            "input_bytes": params.chars.len(),
            "status": if output.exit_code == Some(0) { "success" } else { "active" },
            "stdout_bytes": output.stdout_bytes,
            "stderr_bytes": output.stderr_bytes,
            "truncated": output.truncated,
            "duration_ms": started.elapsed().as_millis(),
        }),
    );
    Ok(output)
}

pub async fn cancel_session(
    state: &AppState,
    actor: &ActorContext,
    params: &crate::tools::params::CancelSessionParams,
) -> Result<CancelSessionOutput, ToolError> {
    ensure_scope(state, actor, crate::policy::WORKSPACE_SHELL)?;
    let status = state
        .runner
        .cancel_session(CancelSessionRequest {
            owner: actor.runner_owner(),
            session_key: Some(actor.principal_id.clone()),
            session_id: params.session_id,
        })
        .await
        .map_err(classify_shell_error)?;
    spawn_tool_audit(
        state,
        actor,
        "session.cancel",
        json!({ "session_id": params.session_id, "status": status.state }),
    );
    Ok(CancelSessionOutput {
        session_id: status.session_id,
        state: status.state,
        exit_code: status.exit_code,
        timed_out: status.timed_out,
        wall_time_seconds: status.wall_time_seconds,
    })
}

pub async fn apply_patch(
    state: &AppState,
    actor: &ActorContext,
    params: &ApplyPatchParams,
) -> Result<ApplyPatchOutput, ToolError> {
    let started = Instant::now();
    ensure_scope(state, actor, crate::policy::WORKSPACE_PATCH)?;
    actor.ensure_write_access().map_err(ToolError::Forbidden)?;
    if actor
        .policy
        .limits
        .max_file_bytes
        .is_some_and(|limit| params.patch_text.len() > limit)
    {
        return Err(ToolError::InvalidInput(
            "patch exceeds resource limit".to_string(),
        ));
    }
    let parsed = parse_patch(&params.patch_text).map_err(map_apply_patch_error)?;
    for operation in parsed.operations {
        match operation {
            PatchOperation::Add { path, .. } | PatchOperation::Delete { path } => {
                validate_workspace_path(&path)?;
            }
            PatchOperation::Update { path, move_to, .. } => {
                validate_workspace_path(&path)?;
                if let Some(move_to) = move_to {
                    validate_workspace_path(&move_to)?;
                }
            }
        }
    }
    let tools = workspace_tools(actor)?;
    let summary = tools
        .apply_patch_text(&params.patch_text)
        .map_err(map_apply_patch_error)?;
    let summary_text = summary.summary.clone();
    let partial = summary.partial;
    let files = summary.changes.len();
    spawn_tool_audit(
        state,
        actor,
        "tool.apply_patch",
        json!({
            "patch_sha256": sha256_hex(&params.patch_text),
            "status": if partial { "partial" } else { "success" },
            "summary": summary_text,
            "files": files,
            "duration_ms": started.elapsed().as_millis(),
        }),
    );
    Ok(summary.into())
}

pub fn read(
    state: &AppState,
    actor: &ActorContext,
    params: &ReadParams,
) -> Result<ReadOutput, ToolError> {
    let started = Instant::now();
    ensure_scope(state, actor, crate::policy::WORKSPACE_READ)?;
    if actor
        .policy
        .limits
        .max_file_bytes
        .is_some_and(|limit| params.limit.unwrap_or(2000).saturating_mul(16 * 1024) > limit)
    {
        return Err(ToolError::InvalidInput(
            "read request exceeds file resource limit".to_string(),
        ));
    }
    validate_workspace_path(&params.file_path)?;
    let output = read_output(&actor.workspace_root, params)?;
    spawn_tool_audit(
        state,
        actor,
        "tool.read",
        json!({
            "path_sha256": sha256_hex(&params.file_path),
            "offset": output.offset,
            "total": output.total,
            "truncated": output.truncated,
            "duration_ms": started.elapsed().as_millis(),
        }),
    );
    Ok(output)
}

pub async fn grep(
    state: &AppState,
    actor: &ActorContext,
    params: &GrepParams,
) -> Result<GrepOutput, ToolError> {
    let started = Instant::now();
    ensure_scope(state, actor, crate::policy::WORKSPACE_SEARCH)?;
    validate_workspace_path(&params.path)?;
    let output = search_files_output_in_runner(state, actor, params).await?;
    spawn_tool_audit(
        state,
        actor,
        "tool.grep",
        json!({
            "pattern_sha256": sha256_hex(&params.pattern),
            "path_sha256": sha256_hex(&params.path),
            "include": params.include,
            "matches": output.matches.len(),
            "truncated": output.truncated,
            "duration_ms": started.elapsed().as_millis(),
        }),
    );
    Ok(output)
}

pub async fn glob(
    state: &AppState,
    actor: &ActorContext,
    params: &GlobParams,
) -> Result<GlobOutput, ToolError> {
    let started = Instant::now();
    ensure_scope(state, actor, crate::policy::WORKSPACE_SEARCH)?;
    validate_workspace_path(&params.path)?;
    let output = glob_output_in_runner(state, actor, params).await?;
    spawn_tool_audit(
        state,
        actor,
        "tool.glob",
        json!({
            "pattern_sha256": sha256_hex(&params.pattern),
            "path_sha256": sha256_hex(&params.path),
            "matches": output.matches.len(),
            "truncated": output.truncated,
            "duration_ms": started.elapsed().as_millis(),
        }),
    );
    Ok(output)
}

pub fn stat_path(
    state: &AppState,
    actor: &ActorContext,
    params: &crate::tools::params::StatPathParams,
) -> Result<StatPathOutput, ToolError> {
    let started = Instant::now();
    ensure_scope(state, actor, crate::policy::WORKSPACE_READ)?;
    validate_workspace_path(&params.path)?;
    let output = stat_path_output(&actor.workspace_root, params)?;
    spawn_tool_audit(
        state,
        actor,
        "tool.stat",
        json!({
            "path_sha256": sha256_hex(&params.path),
            "duration_ms": started.elapsed().as_millis(),
        }),
    );
    Ok(output)
}

fn classify_shell_error(error: anyhow::Error) -> ToolError {
    let message = error.to_string();
    let normalized = message.to_ascii_lowercase();
    match message.as_str() {
        "session does not belong to current user" => {
            ToolError::NotFound("session not found".to_string())
        }
        _ if message.starts_with("unknown session_id ") => {
            ToolError::NotFound("session not found".to_string())
        }
        _ if is_invalid_input_message(&message) => ToolError::InvalidInput(message),
        _ if normalized.contains("no such file or directory")
            && (normalized.contains("working directory")
                || normalized.contains("workdir")
                || normalized.contains("failed to resolve")) =>
        {
            ToolError::NotFound(message)
        }
        _ => ToolError::Internal(error),
    }
}

fn map_apply_patch_error(error: WorkspaceSdkError) -> ToolError {
    map_workspace_sdk_error(error)
}

fn map_workspace_sdk_error(error: WorkspaceSdkError) -> ToolError {
    let message = error.to_string();
    match error {
        WorkspaceSdkError::InvalidInput(message) => ToolError::InvalidInput(message),
        WorkspaceSdkError::PatchContextNotFound(message) => {
            ToolError::PatchContextNotFound(message)
        }
        WorkspaceSdkError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
            ToolError::NotFound(message)
        }
        WorkspaceSdkError::Io { .. } => ToolError::Internal(error.into()),
    }
}

fn is_invalid_input_message(message: &str) -> bool {
    matches!(
        message,
        "absolute paths are not allowed"
            | "path must stay within workspace root"
            | "path escapes workspace root"
    ) || message.starts_with("failed to resolve ")
}
