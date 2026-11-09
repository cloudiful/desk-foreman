use std::time::Instant;

use chrono::Utc;
use desk_foreman_workspace_sdk::{
    PatchCode, PatchDiagnostic, PatchOperation, PatchRejected, PatchResult, PatchStage,
    WorkspaceFileTools, WorkspaceSdkError, parse_patch, patch_path_is_protected,
    resolve_workspace_path,
};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;
use utoipa::ToSchema;

pub use super::fenced_write::{FencedWrite, admit_workspace_write};
pub use crate::tools::common::{
    PatchAppliedOutput, PatchChangeErrorOutput, PatchChangeOutput, PatchCodeOutput,
    PatchDiagnosticOutput, PatchEffectOutput, PatchIncompleteOutput, PatchOperationOutput,
    PatchRejectedOutput, PatchResultOutput, PatchStageOutput, PatchStatusOutput, PatchWritesOutput,
};

use crate::{
    AppState,
    actor::ActorContext,
    git_gateway::{
        capability::{CapabilityAlias, CapabilityClaims, mint},
        capability_expiry, gateway_enabled, remote_url, signing_key,
    },
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
use runner_protocol::{
    CancelSessionRequest, ExecRequest, InputRequest, ShellToolOutput,
    workspace_git::{GitCapabilitySecret, PrepareWorkspaceGitRequest, WorkspaceGitRemote},
};

const DEFAULT_SHELL_TIMEOUT_MS: u64 = 120_000;

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

async fn prepare_workspace_git(state: &AppState, actor: &ActorContext) {
    let Some(binding) = actor.workspace_binding.as_ref() else {
        return;
    };
    let (capability, upstreams, expires_at) = match build_workspace_git(state, actor, binding).await
    {
        Ok(Some(prepared)) => prepared,
        Ok(None) => (None, Vec::new(), 0),
        Err(message) => {
            tracing::warn!("{message}; clearing native Git authorization for this dispatch");
            (None, Vec::new(), 0)
        }
    };
    let request = PrepareWorkspaceGitRequest {
        owner: actor.runner_owner(),
        workspace_root: actor.workspace_root.clone(),
        network_enabled: actor.policy.limits.network_enabled,
        binding_id: binding.workspace_binding_id,
        capability,
        expires_at,
        upstreams,
    };
    if request.is_valid() && state.runner.prepare_workspace_git(request).await.is_err() {
        tracing::warn!(
            "workspace Git preparation failed; native Git stays disabled for this dispatch"
        );
    }
}

type PreparedWorkspaceGit = (Option<GitCapabilitySecret>, Vec<WorkspaceGitRemote>, i64);

async fn build_workspace_git(
    state: &AppState,
    actor: &ActorContext,
    binding: &crate::db::types::WorkspaceBindingResponse,
) -> Result<Option<PreparedWorkspaceGit>, &'static str> {
    if !gateway_enabled(&state.config) {
        return Ok(None);
    }
    let Some(key) = signing_key(&state.config) else {
        return Ok(None);
    };
    let aliases = crate::db::queries::list_gateway_aliases(&state.db, binding.workspace_binding_id)
        .await
        .map_err(|_| "workspace Git alias lookup failed")?;
    let now = Utc::now();
    let expiry = capability_expiry(binding, now);
    let claims = CapabilityClaims {
        v: 1,
        app_id: binding.application_id,
        binding_id: binding.workspace_binding_id,
        external_user_id: actor.external_user_id.clone().unwrap_or_default(),
        principal_id: actor.principal_id.clone(),
        lease_owner: actor.lease_owner.clone().unwrap_or_default(),
        read: true,
        write: actor.ensure_write_access().is_ok(),
        aliases: aliases
            .iter()
            .map(|alias| CapabilityAlias {
                remote_alias: alias.remote_alias.clone(),
                upstream_id: alias.upstream_id,
                generation: alias.capability_generation,
            })
            .collect(),
        iat: now.timestamp(),
        exp: expiry.timestamp(),
    };
    let token = mint(&key, &claims).map_err(|_| "workspace Git capability mint failed")?;
    let urls = aliases
        .iter()
        .map(|alias| {
            remote_url(
                &state.config,
                binding.workspace_binding_id,
                &alias.remote_alias,
            )
            .map(|url| WorkspaceGitRemote {
                remote_alias: alias.remote_alias.clone(),
                url,
            })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or("workspace Git gateway base URL is unavailable")?;
    Ok(Some((
        Some(GitCapabilitySecret::new(token)),
        urls,
        expiry.timestamp(),
    )))
}

fn workspace_tools(actor: &ActorContext) -> Result<WorkspaceFileTools, ToolError> {
    WorkspaceFileTools::new(&actor.workspace_root).map_err(map_workspace_sdk_error)
}

pub async fn shell(
    state: &AppState,
    actor: &ActorContext,
    params: &ShellParams,
) -> Result<ShellToolOutput, ToolError> {
    let started = Instant::now();
    ensure_scope(state, actor, crate::policy::WORKSPACE_SHELL)?;
    let owner = actor.runner_owner();
    if let Some(limit) = actor.policy.limits.max_sessions {
        let active = state
            .runner
            .list_sessions()
            .await
            .map_err(tool_internal)?
            .into_iter()
            .filter(|session| session.owner == owner)
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
    // Admit under the row lock immediately before dispatch and hold the
    // fence across it, so a concurrent takeover cannot commit in between.
    let fence = admit_workspace_write(state, actor).await?;
    prepare_workspace_git(state, actor).await;
    let output = state
        .runner
        .exec_shell(ExecRequest {
            owner: owner.clone(),
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
    fence.commit().await?;
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
    let owner = actor.runner_owner();
    let needs_fence = !params.chars.is_empty();
    // Admit under the row lock immediately before dispatch and hold the
    // fence across it, so a concurrent takeover cannot commit in between.
    // Empty polls only observe the session and stay unfenced.
    let fence = if needs_fence {
        admit_workspace_write(state, actor).await?
    } else {
        FencedWrite { tx: None }
    };
    if needs_fence {
        prepare_workspace_git(state, actor).await;
    }
    let output = state
        .runner
        .write_stdin(InputRequest {
            owner: owner.clone(),
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
    fence.commit().await?;
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
    let owner = actor.runner_owner();
    let status = state
        .runner
        .cancel_session(CancelSessionRequest {
            owner: owner.clone(),
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

#[derive(Clone, Copy)]
enum FenceCommit {
    Real,
    #[cfg(test)]
    Fail,
}

pub async fn apply_patch(
    state: &AppState,
    actor: &ActorContext,
    params: &ApplyPatchParams,
) -> Result<PatchResultOutput, ToolError> {
    apply_patch_inner(state, actor, params, FenceCommit::Real).await
}

#[cfg(test)]
pub(crate) async fn apply_patch_with_commit_failure(
    state: &AppState,
    actor: &ActorContext,
    params: &ApplyPatchParams,
) -> Result<PatchResultOutput, ToolError> {
    apply_patch_inner(state, actor, params, FenceCommit::Fail).await
}

async fn apply_patch_inner(
    state: &AppState,
    actor: &ActorContext,
    params: &ApplyPatchParams,
    commit: FenceCommit,
) -> Result<PatchResultOutput, ToolError> {
    let started = Instant::now();
    ensure_scope(state, actor, crate::policy::WORKSPACE_PATCH)?;
    // Fenced admission covers direct workspace-SDK file writes as well as
    // runner-backed writes. Syntax and protected-path validation runs first so
    // those rejections never take the row lock; admission then holds the lock
    // across the filesystem write so a request admitted under an old owner
    // cannot cross a handover.
    if actor
        .policy
        .limits
        .max_file_bytes
        .is_some_and(|limit| params.patch_text.len() > limit)
    {
        return Ok(rejected(PatchDiagnostic::new(
            PatchCode::PatchTooLarge,
            PatchStage::Params,
        )));
    }
    let parsed = match parse_patch(&params.patch_text) {
        Ok(parsed) => parsed,
        Err(diagnostic) => return Ok(rejected(diagnostic)),
    };
    for operation in &parsed.operations {
        let (path, operation, move_to) = match operation {
            PatchOperation::Add { path, .. } => (
                path,
                desk_foreman_workspace_sdk::ApplyPatchOperation::Add,
                None,
            ),
            PatchOperation::Delete { path } => (
                path,
                desk_foreman_workspace_sdk::ApplyPatchOperation::Delete,
                None,
            ),
            PatchOperation::Update { path, move_to, .. } => (
                path,
                if move_to.is_some() {
                    desk_foreman_workspace_sdk::ApplyPatchOperation::Move
                } else {
                    desk_foreman_workspace_sdk::ApplyPatchOperation::Update
                },
                move_to.as_deref(),
            ),
        };
        if resolve_workspace_path(&actor.workspace_root, path).is_err() {
            return Ok(rejected(
                PatchDiagnostic::new(PatchCode::PatchPathInvalid, PatchStage::Plan)
                    .with_operation(Some(operation)),
            ));
        }
        if patch_path_is_protected(path) {
            return Ok(rejected(
                PatchDiagnostic::new(PatchCode::PatchPathProtected, PatchStage::Plan)
                    .with_path(Some(path.clone()))
                    .with_operation(Some(operation)),
            ));
        }
        if let Some(move_to) = move_to {
            if resolve_workspace_path(&actor.workspace_root, move_to).is_err() {
                return Ok(rejected(
                    PatchDiagnostic::new(PatchCode::PatchPathInvalid, PatchStage::Plan)
                        .with_operation(Some(operation)),
                ));
            }
            if patch_path_is_protected(move_to) {
                return Ok(rejected(
                    PatchDiagnostic::new(PatchCode::PatchPathProtected, PatchStage::Plan)
                        .with_path(Some(move_to.to_string()))
                        .with_operation(Some(operation)),
                ));
            }
        }
    }
    let fence = admit_workspace_write(state, actor).await?;
    let tools = workspace_tools(actor)?;
    let result = tools.apply_patch_text(&params.patch_text);
    let commit_result = match commit {
        FenceCommit::Real => fence.commit().await,
        #[cfg(test)]
        FenceCommit::Fail => {
            drop(fence);
            Err(tool_internal(anyhow::anyhow!(
                "injected patch fence commit failure"
            )))
        }
    };
    let result = match commit_result {
        Ok(()) => result,
        Err(_) => return Ok(fence_failure_result(result).into()),
    };
    let status = match &result {
        PatchResult::Applied(_) => "success",
        PatchResult::Rejected(_) => "rejected",
        PatchResult::Incomplete(_) => "partial",
    };
    let files = match &result {
        PatchResult::Applied(applied) => applied.changes.len(),
        PatchResult::Rejected(_) => 0,
        PatchResult::Incomplete(incomplete) => incomplete.changes.len(),
    };
    spawn_tool_audit(
        state,
        actor,
        "tool.apply_patch",
        json!({
            "patch_sha256": sha256_hex(&params.patch_text),
            "status": status,
            "files": files,
            "duration_ms": started.elapsed().as_millis(),
        }),
    );
    Ok(result.into())
}

fn rejected(diagnostic: PatchDiagnostic) -> PatchResultOutput {
    PatchResult::Rejected(PatchRejected { diagnostic }).into()
}

/// A fence commit can fail after all writes were already published. Only an
/// applied result is reclassified, and it keeps the actual applied changes; an
/// existing incomplete result keeps its first truthful diagnostic and effects.
pub(crate) fn fence_failure_result(result: PatchResult) -> PatchResult {
    match result {
        PatchResult::Applied(applied) => {
            let writes = desk_foreman_workspace_sdk::classify_writes(&applied.changes);
            PatchResult::Incomplete(desk_foreman_workspace_sdk::PatchIncomplete {
                writes,
                changes: applied.changes,
                diagnostic: PatchDiagnostic::new(PatchCode::PatchFenceFailed, PatchStage::Fence),
            })
        }
        other => other,
    }
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

fn map_workspace_sdk_error(error: WorkspaceSdkError) -> ToolError {
    let message = error.to_string();
    match error {
        WorkspaceSdkError::InvalidInput(message) => ToolError::InvalidInput(message),
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
