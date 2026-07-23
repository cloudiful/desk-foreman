use std::{
    borrow::Cow,
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::Arc,
};

use rmcp::{
    handler::server::{tool::schema_for_output, wrapper::Parameters},
    model::{CallToolResult, Content, JsonObject, Tool, ToolAnnotations},
};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;
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

pub(super) fn readonly_tool<I, O>(name: &'static str, description: &'static str) -> Tool
where
    I: JsonSchema + 'static,
    O: JsonSchema + 'static,
{
    let output_schema = schema_for_output::<O>().expect("output schema must be available");
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
    let actor_type = match actor.mode {
        ActorMode::InternalUser => "user",
        ActorMode::ApplicationSubject => "application",
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

pub(super) fn mcp_error(error: ToolError) -> rmcp::ErrorData {
    match error {
        ToolError::InvalidInput(message)
        | ToolError::NotFound(message)
        | ToolError::Forbidden(message) => rmcp::ErrorData::invalid_params(message, None),
        ToolError::Internal(error) => internal_error(error),
    }
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
    state
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
        .map_err(tool_internal)
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

    let mut result = CallToolResult::success(vec![Content::text(text)]);
    result.structured_content = Some(structured);
    result.is_error = Some(false);
    result
}

pub(super) fn structured_text_result<T: Serialize>(
    text: String,
    output: &T,
) -> Result<CallToolResult, rmcp::ErrorData> {
    let structured = serde_json::to_value(output).map_err(internal_error)?;
    let mut result = CallToolResult::success(vec![Content::text(text)]);
    result.structured_content = Some(structured);
    result.is_error = Some(false);
    Ok(result)
}
