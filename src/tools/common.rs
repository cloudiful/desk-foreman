use std::{
    borrow::Cow,
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::Arc,
};

use rmcp::{
    handler::server::{tool::schema_for_output, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, JsonObject, Tool, ToolAnnotations},
};
use schemars::{JsonSchema, Schema};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;
use validator::Validate;

use crate::{
    AppState,
    actor::{ActorContext, ActorMode},
    api::validation::validation_errors_message,
    db::{self, audit::AuditLogEntry},
};
use runner_protocol::{CommandOutput, RunnerCommandRequest, ShellToolOutput};
use utoipa::ToSchema;

use desk_foreman_workspace_sdk::{
    ApplyPatchOperation, ApplyPatchStatus, PatchCode, PatchDiagnostic, PatchEffect, PatchResult,
    PatchStage, PatchWrites,
};

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("{0}")]
    InvalidInput(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Forbidden(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

/// Every field of these always-serialized patch objects is part of the wire
/// contract, including null-valued optionals. Derive the required names from
/// the generated properties so the schema cannot drift from the object shape.
fn require_all_object_properties(schema: &mut Schema) {
    let object = schema
        .as_object_mut()
        .unwrap_or_else(|| panic!("patch object schema must be an object"));
    let properties = object
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("patch object schema must contain object properties"));
    let required = properties.keys().cloned().map(Value::String).collect();
    object.insert("required".to_string(), Value::Array(required));
}

/// Strict wire result shared by the MCP and HTTP patch tools.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PatchResultOutput {
    Applied(PatchAppliedOutput),
    Rejected(PatchRejectedOutput),
    Incomplete(PatchIncompleteOutput),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct PatchAppliedOutput {
    #[schemars(length(min = 1))]
    #[schema(min_items = 1)]
    pub changes: Vec<PatchChangeOutput>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct PatchRejectedOutput {
    pub diagnostic: PatchDiagnosticOutput,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct PatchIncompleteOutput {
    pub writes: PatchWritesOutput,
    pub changes: Vec<PatchChangeOutput>,
    pub diagnostic: PatchDiagnosticOutput,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields, transform = require_all_object_properties)]
pub struct PatchChangeOutput {
    pub operation: PatchOperationOutput,
    pub path: String,
    #[schema(required = true)]
    pub destination: Option<String>,
    pub status: PatchStatusOutput,
    pub effect: PatchEffectOutput,
    #[schema(required = true)]
    pub error: Option<PatchChangeErrorOutput>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct PatchChangeErrorOutput {
    pub code: PatchCodeOutput,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields, transform = require_all_object_properties)]
pub struct PatchDiagnosticOutput {
    pub code: PatchCodeOutput,
    pub stage: PatchStageOutput,
    #[schema(required = true)]
    pub path: Option<String>,
    #[schema(required = true)]
    pub operation: Option<PatchOperationOutput>,
    #[schema(required = true)]
    pub hunk: Option<u32>,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PatchOperationOutput {
    Add,
    Delete,
    Update,
    Move,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PatchStatusOutput {
    Applied,
    Failed,
    Skipped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PatchEffectOutput {
    Changed,
    Unchanged,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PatchWritesOutput {
    None,
    All,
    Partial,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PatchStageOutput {
    Params,
    Parse,
    Plan,
    Commit,
    Fence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PatchCodeOutput {
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

impl From<PatchResult> for PatchResultOutput {
    fn from(result: PatchResult) -> Self {
        match result {
            PatchResult::Applied(applied) => Self::Applied(PatchAppliedOutput {
                changes: applied.changes.into_iter().map(Into::into).collect(),
            }),
            PatchResult::Rejected(rejected) => Self::Rejected(PatchRejectedOutput {
                diagnostic: rejected.diagnostic.into(),
            }),
            PatchResult::Incomplete(incomplete) => Self::Incomplete(PatchIncompleteOutput {
                writes: incomplete.writes.into(),
                changes: incomplete.changes.into_iter().map(Into::into).collect(),
                diagnostic: incomplete.diagnostic.into(),
            }),
        }
    }
}

impl From<ApplyPatchOperation> for PatchOperationOutput {
    fn from(operation: ApplyPatchOperation) -> Self {
        match operation {
            ApplyPatchOperation::Add => Self::Add,
            ApplyPatchOperation::Delete => Self::Delete,
            ApplyPatchOperation::Update => Self::Update,
            ApplyPatchOperation::Move => Self::Move,
        }
    }
}

impl From<ApplyPatchStatus> for PatchStatusOutput {
    fn from(status: ApplyPatchStatus) -> Self {
        match status {
            ApplyPatchStatus::Applied => Self::Applied,
            ApplyPatchStatus::Failed => Self::Failed,
            ApplyPatchStatus::Skipped => Self::Skipped,
        }
    }
}

impl From<PatchEffect> for PatchEffectOutput {
    fn from(effect: PatchEffect) -> Self {
        match effect {
            PatchEffect::Changed => Self::Changed,
            PatchEffect::Unchanged => Self::Unchanged,
            PatchEffect::Unknown => Self::Unknown,
        }
    }
}

impl From<PatchWrites> for PatchWritesOutput {
    fn from(writes: PatchWrites) -> Self {
        match writes {
            PatchWrites::None => Self::None,
            PatchWrites::All => Self::All,
            PatchWrites::Partial => Self::Partial,
            PatchWrites::Unknown => Self::Unknown,
        }
    }
}

impl From<PatchStage> for PatchStageOutput {
    fn from(stage: PatchStage) -> Self {
        match stage {
            PatchStage::Params => Self::Params,
            PatchStage::Parse => Self::Parse,
            PatchStage::Plan => Self::Plan,
            PatchStage::Commit => Self::Commit,
            PatchStage::Fence => Self::Fence,
        }
    }
}

impl From<PatchCode> for PatchCodeOutput {
    fn from(code: PatchCode) -> Self {
        match code {
            PatchCode::PatchArgInvalid => Self::PatchArgInvalid,
            PatchCode::PatchTooLarge => Self::PatchTooLarge,
            PatchCode::PatchBoundaryInvalid => Self::PatchBoundaryInvalid,
            PatchCode::PatchHunkInvalid => Self::PatchHunkInvalid,
            PatchCode::PatchPathInvalid => Self::PatchPathInvalid,
            PatchCode::PatchPathProtected => Self::PatchPathProtected,
            PatchCode::PatchTargetMissing => Self::PatchTargetMissing,
            PatchCode::PatchTargetExists => Self::PatchTargetExists,
            PatchCode::PatchTargetNotRegular => Self::PatchTargetNotRegular,
            PatchCode::PatchNotUtf8Text => Self::PatchNotUtf8Text,
            PatchCode::PatchContextNotFound => Self::PatchContextNotFound,
            PatchCode::PatchContextAmbiguous => Self::PatchContextAmbiguous,
            PatchCode::PatchMoveConflict => Self::PatchMoveConflict,
            PatchCode::PatchDuplicateTarget => Self::PatchDuplicateTarget,
            PatchCode::PatchTargetChanged => Self::PatchTargetChanged,
            PatchCode::PatchSourceChanged => Self::PatchSourceChanged,
            PatchCode::PatchWriteFailed => Self::PatchWriteFailed,
            PatchCode::PatchFenceFailed => Self::PatchFenceFailed,
        }
    }
}

impl From<desk_foreman_workspace_sdk::PatchChange> for PatchChangeOutput {
    fn from(change: desk_foreman_workspace_sdk::PatchChange) -> Self {
        Self {
            operation: change.operation.into(),
            path: change.path,
            destination: change.destination,
            status: change.status.into(),
            effect: change.effect.into(),
            error: change.error.map(Into::into),
        }
    }
}

impl From<desk_foreman_workspace_sdk::PatchChangeError> for PatchChangeErrorOutput {
    fn from(error: desk_foreman_workspace_sdk::PatchChangeError) -> Self {
        Self {
            code: error.code.into(),
            message: error.message().to_string(),
        }
    }
}

impl From<PatchDiagnostic> for PatchDiagnosticOutput {
    fn from(diagnostic: PatchDiagnostic) -> Self {
        let message = diagnostic.message().to_string();
        Self {
            code: diagnostic.code.into(),
            stage: diagnostic.stage.into(),
            path: diagnostic.path,
            operation: diagnostic.operation.map(Into::into),
            hunk: diagnostic.hunk,
            message,
        }
    }
}

pub(super) fn patch_call_result(
    output: &PatchResultOutput,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let structured = serde_json::to_value(output).map_err(internal_error)?;
    let text = serde_json::to_string(output).map_err(internal_error)?;
    let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
    result.structured_content = Some(structured);
    result.is_error = Some(!matches!(output, PatchResultOutput::Applied(_)));
    Ok(result)
}

pub(super) fn readonly_tool<I, O>(name: &'static str, description: &'static str) -> Tool
where
    I: JsonSchema + 'static,
    O: JsonSchema + 'static,
{
    let output_schema = schema_for_output::<O>();
    Tool::new_with_raw(
        name,
        Some(Cow::Borrowed(description)),
        schema_for_input::<I>(),
    )
    .with_annotations(readonly_annotations())
    .with_raw_output_schema(output_schema)
}

pub(super) fn write_annotations() -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(false)
        .destructive(true)
        .idempotent(false)
        .open_world(false)
}

fn readonly_annotations() -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(true)
        .destructive(false)
        .idempotent(true)
        .open_world(false)
}

pub(super) fn schema_for_input<T: JsonSchema + 'static>()
-> Arc<serde_json::Map<String, serde_json::Value>> {
    rmcp::handler::server::tool::schema_for_type::<Parameters<T>>()
}

pub(super) fn internal_error<E>(error: E) -> rmcp::ErrorData
where
    E: Into<anyhow::Error>,
{
    rmcp::ErrorData::internal_error(error.into().to_string(), None)
}

pub(super) fn tool_invalid_input<E>(error: E) -> ToolError
where
    E: Into<anyhow::Error>,
{
    ToolError::InvalidInput(error.into().to_string())
}

pub(super) fn tool_internal<E>(error: E) -> ToolError
where
    E: Into<anyhow::Error>,
{
    ToolError::Internal(error.into())
}

pub(super) fn validate_shell_command(command: &str) -> Result<(), ToolError> {
    let dangerous_command = command
        .split([' ', '\t', '\n', '\r', ';', '|', '&', '>', '<'])
        .any(is_dangerous_command_token);
    if dangerous_command
        || command.contains("/var/run/docker.sock")
        || command.contains("rm -rf /")
        || command.contains("kill -9 1")
        || command
            .split([' ', '\t', '\n', '\r', ';', '|', '&', '>', '<'])
            .any(is_sensitive_path_token)
    {
        return Err(ToolError::Forbidden(
            "command is not allowed by workspace policy".to_string(),
        ));
    }
    Ok(())
}

fn is_dangerous_command_token(token: &str) -> bool {
    let token = token.trim_matches(|character| matches!(character, '\'' | '"' | '`' | '(' | ')'));
    let base = Path::new(token)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(token)
        .to_ascii_lowercase();
    matches!(
        base.as_str(),
        "docker"
            | "podman"
            | "sudo"
            | "su"
            | "mount"
            | "umount"
            | "mkfs"
            | "shutdown"
            | "reboot"
            | "poweroff"
    )
}

pub(super) fn validate_shell_binary(shell: &str) -> Result<(), ToolError> {
    let base = Path::new(shell)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(shell);
    if !matches!(base, "bash" | "sh") {
        return Err(ToolError::Forbidden(
            "shell binary is not allowed by workspace policy".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn validate_workspace_path(path: &str) -> Result<(), ToolError> {
    let normalized = path.replace('\\', "/");
    let sensitive_name = normalized
        .split('/')
        .any(|part| part == ".env" || part.starts_with(".env.") || part == "credentials");
    let sensitive_extension = [".pem", ".key", ".p12", ".pfx"]
        .iter()
        .any(|suffix| normalized.ends_with(suffix));
    let sensitive_basename = ["id_rsa", "id_ed25519", "authorized_keys"]
        .iter()
        .any(|name| normalized.ends_with(name));
    if sensitive_name || sensitive_extension || sensitive_basename {
        return Err(ToolError::Forbidden(
            "path is protected by workspace policy".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn ensure_scope(
    state: &AppState,
    actor: &ActorContext,
    scope: &'static str,
) -> Result<(), ToolError> {
    if actor.policy.allows(scope) {
        return Ok(());
    }
    spawn_tool_audit(state, actor, "policy.deny", json!({ "scope": scope }));
    Err(ToolError::Forbidden(format!(
        "missing required scope {scope}"
    )))
}

fn is_sensitive_path_token(token: &str) -> bool {
    let token = token.trim_matches(|character| matches!(character, '\'' | '"' | '`' | '(' | ')'));
    let normalized = token.replace('\\', "/");
    normalized.split('/').any(|part| {
        part == ".env"
            || part.starts_with(".env.")
            || part == ".ssh"
            || part == ".aws"
            || part == ".kube"
            || part == ".docker"
            || part == "credentials"
            || part == "id_rsa"
            || part == "id_ed25519"
            || part == "authorized_keys"
    }) || [".pem", ".key", ".p12", ".pfx"]
        .iter()
        .any(|suffix| normalized.ends_with(suffix))
}

pub(super) fn sha256_hex(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn spawn_tool_audit(
    state: &AppState,
    actor: &ActorContext,
    action: &'static str,
    payload: serde_json::Value,
) {
    let Some(handle) = tokio::runtime::Handle::try_current().ok() else {
        return;
    };
    let Ok(pool) = catch_unwind(AssertUnwindSafe(|| state.db.clone())) else {
        return;
    };
    let actor_user_id = actor.user.as_ref().map(|user| user.user_id);
    let actor_application_id = actor.application.as_ref().map(|app| app.application_id);
    let actor_type = match &actor.mode {
        ActorMode::InternalUser => "user",
        ActorMode::ApplicationSubject | ActorMode::IsolatedApplicationPrincipal(_) => "application",
    };
    let target_id = actor.principal_id.clone();
    let workspace_binding_id = Some(actor.workspace_binding_id);
    let external_user_id = actor.external_user_id.clone();
    handle.spawn(async move {
        let status = payload
            .get("status")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let duration_ms = payload
            .get("duration_ms")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| i64::try_from(value).ok());
        let session_id = payload
            .get("session_id")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| i64::try_from(value).ok());
        if let Err(error) = db::queries::record_audit(
            &pool,
            AuditLogEntry {
                actor_user_id,
                actor_application_id,
                actor_type,
                action,
                target_type: "workspace_tool",
                target_id: &target_id,
                workspace_binding_id,
                external_user_id: external_user_id.as_deref(),
                payload,
                request_id: None,
                session_id,
                duration_ms,
                status: status.as_deref(),
            },
        )
        .await
        {
            tracing::warn!(%error, action, "failed to record workspace tool audit");
        }
    });
}

pub(super) fn tool_error_result(error: ToolError) -> Result<CallToolResult, rmcp::ErrorData> {
    let (code, category, message, repairable, suggested_action) = match error {
        ToolError::InvalidInput(message) => (
            "invalid_input",
            "tool_input",
            message,
            true,
            "fix_arguments_and_retry",
        ),
        ToolError::NotFound(message) => (
            "not_found",
            "tool_input",
            message,
            true,
            "inspect_workspace_and_retry",
        ),
        ToolError::Forbidden(message) => ("forbidden", "policy", message, false, "do_not_retry"),
        ToolError::Internal(error) => return Err(internal_error(error)),
    };
    Ok(CallToolResult::structured_error(json!({
        "ok": false,
        "error": {
            "code": code,
            "category": category,
            "message": message,
            "repairable": repairable,
            "retryable": false,
            "suggested_action": suggested_action,
            "task_id": null,
            "details": null,
            "trace_id": null,
        }
    })))
}

pub(super) fn parse_and_validate_tool_params<T>(input: JsonObject) -> Result<T, rmcp::ErrorData>
where
    T: serde::de::DeserializeOwned + Validate,
{
    let value: T = rmcp::handler::server::tool::parse_json_object(input)?;
    value.validate().map_err(|errors| {
        rmcp::ErrorData::invalid_params(validation_errors_message(&errors), None)
    })?;
    Ok(value)
}

pub(super) async fn run_command_in_runner(
    state: &crate::AppState,
    actor: &crate::actor::ActorContext,
    workdir: std::path::PathBuf,
    program: &str,
    args: Vec<String>,
) -> Result<CommandOutput, ToolError> {
    // Read-only container-backed tools still create a runner container, so
    // they take the same fresh binding row lock as writes. The lock is held
    // through the runner call (including the first Running acknowledgement)
    // and released only after a successful dispatch; an error drops the guard
    // and rolls the lock back. Reads acquire no write lease.
    let fence = crate::tools::fenced_read::admit_workspace_read(state, actor).await?;
    let output = state
        .runner
        .run_command(RunnerCommandRequest {
            owner: actor.runner_owner(),
            workspace_root: actor.workspace_root.clone(),
            working_dir: workdir,
            program: program.to_string(),
            args,
            timeout_ms: actor.policy.limits.max_timeout_ms.or(Some(120_000)),
            max_output_bytes: actor.policy.limits.max_output_bytes,
            network_enabled: actor.policy.limits.network_enabled,
        })
        .await
        .map_err(tool_internal)?;
    fence.commit().await?;
    Ok(output)
}

pub(super) fn shell_call_result(output: ShellToolOutput) -> CallToolResult {
    let structured = serde_json::to_value(&output).expect("ShellToolOutput serializes");
    let text = if output.output.is_empty() {
        json!({
            "session_id": output.session_id,
            "exit_code": output.exit_code,
            "wall_time_seconds": output.wall_time_seconds,
        })
        .to_string()
    } else {
        output.output.clone()
    };

    let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
    result.structured_content = Some(structured);
    result.is_error = Some(false);
    result
}

pub(super) fn structured_text_result<T: Serialize>(
    text: String,
    output: &T,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let structured = serde_json::to_value(output).map_err(internal_error)?;
    let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
    result.structured_content = Some(structured);
    result.is_error = Some(false);
    Ok(result)
}
