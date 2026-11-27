use super::{Entry, ExecutionState};
use crate::{
    AppState,
    actor::ActorContext,
    tools::{
        ToolError,
        common::{ensure_scope, sha256_hex, spawn_tool_audit},
        params::ShellParams,
        shared::{
            admit_workspace_write, prepare_workspace_git_on, shell_preflight,
            shell_session_admission,
        },
    },
};
use std::time::Instant;

use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use runner_protocol::{
    RetainedSessionState, RetainedSessionStatus, SessionInputRequest, SessionLookupRequest,
    SessionOutputChunk, SessionOutputRequest,
};
use serde::Serialize;

#[derive(Debug)]
pub struct StartError {
    pub handle: Option<String>,
    pub error: ToolError,
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct Descriptor {
    #[serde(rename = "execution_id")]
    pub handle: String,
    pub state: String,
    pub exit_code: Option<i32>,
    pub output_complete: bool,
    pub timed_out: bool,
    pub cancelled: bool,
    pub stdout_bytes: Option<u64>,
    pub stderr_bytes: Option<u64>,
    pub dropped_bytes: Option<u64>,
    pub truncated: Option<bool>,
    pub created_at: String,
    pub updated_at: String,
    pub duration_ms: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("execution output unavailable")]
    Unavailable,
    #[error(transparent)]
    Tool(#[from] ToolError),
}

#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct Output {
    pub encoding: &'static str,
    pub stdout_base64: String,
    pub stderr_base64: String,
    pub next_cursor: String,
    pub eof: bool,
    pub output_complete: bool,
    pub trimmed: bool,
    pub truncated: bool,
    pub dropped_bytes: u64,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
}

impl From<SessionOutputChunk> for Output {
    fn from(chunk: SessionOutputChunk) -> Self {
        Self {
            encoding: "base64",
            stdout_base64: STANDARD.encode(chunk.stdout),
            stderr_base64: STANDARD.encode(chunk.stderr),
            next_cursor: chunk.next_cursor,
            eof: chunk.eof,
            output_complete: chunk.output_complete,
            trimmed: chunk.trimmed,
            truncated: chunk.truncated,
            dropped_bytes: chunk.dropped_bytes,
            stdout_bytes: chunk.stdout_bytes,
            stderr_bytes: chunk.stderr_bytes,
        }
    }
}

fn wrap_cursor(handle: &str, cursor: &str) -> Result<String, ToolError> {
    let value = format!("v1:{handle}:{}", URL_SAFE_NO_PAD.encode(cursor));
    if value.len() > 128 {
        return Err(retryable());
    }
    Ok(value)
}

fn unwrap_cursor(handle: &str, value: &str) -> Result<String, ToolError> {
    let invalid = || ToolError::InvalidInput("invalid output cursor".into());
    if value.len() > 128 {
        return Err(invalid());
    }
    let mut parts = value.split(':');
    if parts.next() != Some("v1") || parts.next() != Some(handle) {
        return Err(invalid());
    }
    let encoded = parts.next().ok_or_else(invalid)?;
    if encoded.is_empty() || parts.next().is_some() {
        return Err(invalid());
    }
    String::from_utf8(URL_SAFE_NO_PAD.decode(encoded).map_err(|_| invalid())?)
        .map_err(|_| invalid())
}

fn public_output(handle: &str, chunk: SessionOutputChunk) -> Result<Output, ToolError> {
    let mut output: Output = chunk.into();
    output.next_cursor = wrap_cursor(handle, &output.next_cursor)?;
    Ok(output)
}

pub(super) fn descriptor(handle: &str, entry: &Entry) -> Descriptor {
    let (state, exit_code, complete) = match &entry.state {
        ExecutionState::Starting => ("starting", None, false),
        ExecutionState::Running => ("running", None, false),
        ExecutionState::Uncertain => ("uncertain", None, false),
        ExecutionState::Terminal(status) => (
            match status.state {
                RetainedSessionState::Running => "running",
                RetainedSessionState::Exited => "exited",
                RetainedSessionState::Cancelled => "cancelled",
                RetainedSessionState::TimedOut => "timed_out",
            },
            status.exit_code,
            status.output_complete,
        ),
    };
    Descriptor {
        handle: handle.into(),
        state: state.into(),
        exit_code,
        output_complete: complete,
        timed_out: state == "timed_out",
        cancelled: state == "cancelled",
        stdout_bytes: entry.observed.as_ref().map(|status| status.stdout_bytes),
        stderr_bytes: entry.observed.as_ref().map(|status| status.stderr_bytes),
        dropped_bytes: entry.observed.as_ref().map(|status| status.dropped_bytes),
        truncated: entry
            .observed
            .as_ref()
            .map(|status| status.dropped_bytes > 0),
        created_at: entry.created_at.to_rfc3339(),
        updated_at: entry.updated_at.to_rfc3339(),
        duration_ms: u64::try_from(
            entry
                .terminal_at
                .unwrap_or_else(std::time::Instant::now)
                .saturating_duration_since(entry.started_at)
                .as_millis(),
        )
        .unwrap_or(u64::MAX),
    }
}

fn lookup(state: &AppState, actor: &ActorContext, handle: &str) -> Result<Entry, ToolError> {
    ensure_scope(state, actor, crate::policy::WORKSPACE_SHELL)?;
    state
        .executions
        .get(
            handle,
            &actor.principal_id,
            actor.workspace_binding_id,
            &actor.runner_owner(),
        )
        .ok_or_else(|| ToolError::NotFound("execution not found".into()))
}

fn retained_request(
    owner: runner_protocol::RunnerOwner,
    principal: &str,
    handle: &str,
    session_id: u64,
) -> SessionLookupRequest {
    SessionLookupRequest {
        owner,
        session_key: Some(principal.to_owned()),
        execution_key: handle.to_owned(),
        session_id,
    }
}
fn request(actor: &ActorContext, handle: &str, session_id: u64) -> SessionLookupRequest {
    retained_request(
        actor.runner_owner(),
        &actor.principal_id,
        handle,
        session_id,
    )
}
fn retryable() -> ToolError {
    ToolError::Internal(anyhow::anyhow!("runner temporarily unavailable"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum InputStage {
    Validation,
    Lookup,
    State,
    SessionMissing,
    FenceAdmit,
    ManagerDispatch,
    FenceCommit,
}

impl InputStage {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "validation",
            Self::Lookup => "lookup",
            Self::State => "state",
            Self::SessionMissing => "session_missing",
            Self::FenceAdmit => "fence_admit",
            Self::ManagerDispatch => "manager_dispatch",
            Self::FenceCommit => "fence_commit",
        }
    }

    pub(super) fn dispatch_attempted(self) -> bool {
        matches!(self, Self::ManagerDispatch | Self::FenceCommit)
    }
}

struct InputFailure {
    stage: InputStage,
    error: ToolError,
}

impl InputFailure {
    fn new(stage: InputStage, error: ToolError) -> Self {
        Self { stage, error }
    }
}

pub(super) fn warn_input_failure(stage: InputStage, started: Instant) {
    tracing::warn!(
        stage = stage.as_str(),
        elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        dispatch_attempted = stage.dispatch_attempted(),
        "workspace execution input failed"
    );
}

fn missing(error: &anyhow::Error, session: u64) -> bool {
    error.to_string() == format!("unknown session_id {session}")
}

struct StartGuard<'a> {
    registry: &'a super::ExecutionRegistry,
    handle: String,
    dispatched: bool,
}
impl Drop for StartGuard<'_> {
    fn drop(&mut self) {
        if !self.dispatched {
            self.registry.remove_starting(&self.handle);
            return;
        }
        self.registry.update(&self.handle, |entry| {
            if matches!(entry.state, ExecutionState::Starting) {
                entry.mark_uncertain(None);
            }
        });
    }
}

struct DispatchedWriteGuard<'a> {
    registry: &'a super::ExecutionRegistry,
    handle: &'a str,
    armed: bool,
    evidence: Option<RetainedSessionStatus>,
}

impl Drop for DispatchedWriteGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.registry.update(self.handle, |entry| {
                entry.mark_uncertain(self.evidence.take())
            });
        }
    }
}

fn note_missing(state: &AppState, handle: &str) {
    state.executions.update(handle, |entry| {
        if matches!(entry.state, ExecutionState::Running) {
            entry.mark_uncertain(None);
        }
    });
}

pub async fn start(
    state: &AppState,
    actor: &ActorContext,
    params: &ShellParams,
) -> Result<(Descriptor, Output), StartError> {
    let pre = |error| StartError {
        handle: None,
        error,
    };
    let mut command = shell_preflight(state, actor, params).map_err(pre)?;
    command.yield_time_ms = Some(0);
    let manager = crate::db::queries::find_enabled_runner_manager(&state.db)
        .await
        .map_err(|_| pre(retryable()))?
        .ok_or_else(|| pre(retryable()))?
        .runner_manager_id;
    let _start_guard = state.executions.start_gate.lock().await;
    shell_session_admission(state, actor, Some(manager))
        .await
        .map_err(pre)?;
    let fence = admit_workspace_write(state, actor).await.map_err(pre)?;
    let handle = state
        .executions
        .reserve(
            &actor.principal_id,
            actor.workspace_binding_id,
            actor.runner_owner(),
            manager,
        )
        .map_err(|message| pre(ToolError::Forbidden(message.into())))?;
    let mut dispatch_guard = StartGuard {
        registry: &state.executions,
        handle: handle.clone(),
        dispatched: false,
    };
    if let Err(error) = prepare_workspace_git_on(state, actor, Some(manager)).await {
        return Err(pre(error));
    }
    dispatch_guard.dispatched = true;
    let result = state
        .runner
        .exec_shell_session_for_manager(
            manager,
            runner_protocol::ExecSessionRequest {
                execution_key: handle.clone(),
                request: command,
            },
        )
        .await;
    let started = match result {
        Ok(started) => started,
        Err(_) => {
            state
                .executions
                .update(&handle, |entry| entry.mark_uncertain(None));
            return Err(StartError {
                handle: Some(handle),
                error: retryable(),
            });
        }
    };
    state
        .executions
        .update(&handle, |entry| entry.session = Some(started.session_id));
    if fence.commit().await.is_err() {
        state
            .executions
            .update(&handle, |entry| entry.mark_uncertain(None));
        return Err(StartError {
            handle: Some(handle),
            error: retryable(),
        });
    }
    state.executions.update(&handle, |entry| {
        entry.observe(RetainedSessionStatus {
            state: RetainedSessionState::Running,
            exit_code: None,
            output_complete: false,
            stdout_bytes: started.chunk.stdout_bytes,
            stderr_bytes: started.chunk.stderr_bytes,
            dropped_bytes: started.chunk.dropped_bytes,
        })
    });
    let entry = lookup(state, actor, &handle).map_err(|error| StartError {
        handle: Some(handle.clone()),
        error,
    })?;
    spawn_tool_audit(
        state,
        actor,
        "execution.start",
        serde_json::json!({"command_sha256": sha256_hex(&params.command), "status": "started"}),
    );
    let output = public_output(&handle, started.chunk).map_err(|error| StartError {
        handle: Some(handle.clone()),
        error,
    })?;
    Ok((descriptor(&handle, &entry), output))
}

pub async fn status(
    state: &AppState,
    actor: &ActorContext,
    handle: &str,
) -> Result<Descriptor, ToolError> {
    let gate = lookup(state, actor, handle)?.op_gate;
    let _guard = gate.lock().await;
    let entry = lookup(state, actor, handle)?;
    if matches!(
        entry.state,
        ExecutionState::Uncertain | ExecutionState::Starting
    ) {
        return Ok(descriptor(handle, &entry));
    }
    let session = entry.session.ok_or_else(retryable)?;
    let response = match state
        .runner
        .session_status_for_manager(entry.manager, request(actor, handle, session))
        .await
    {
        Ok(response) => response,
        Err(error) if missing(&error, session) => {
            note_missing(state, handle);
            return Ok(descriptor(handle, &lookup(state, actor, handle)?));
        }
        Err(_) => return Err(retryable()),
    };
    if let Some(status) = response.status {
        state
            .executions
            .update(handle, |entry| entry.observe(status));
    } else {
        state.executions.update(handle, |entry| {
            if matches!(entry.state, ExecutionState::Running) {
                entry.mark_uncertain(None)
            }
        });
    }
    Ok(descriptor(handle, &lookup(state, actor, handle)?))
}

pub async fn output(
    state: &AppState,
    actor: &ActorContext,
    handle: &str,
    cursor: Option<String>,
    max_bytes: usize,
) -> Result<Output, OutputError> {
    let entry = lookup(state, actor, handle)?;
    if entry
        .terminal_at
        .is_some_and(|at| at.elapsed() >= std::time::Duration::from_secs(300))
    {
        return Err(OutputError::Unavailable);
    }
    let session = entry.session.ok_or_else(retryable)?;
    if max_bytes == 0 {
        return Err(ToolError::InvalidInput("invalid output cap".into()).into());
    }
    let cap = max_bytes
        .min(actor.policy.limits.max_output_bytes.unwrap_or(1024 * 1024))
        .min(1024 * 1024);
    if cap == 0 {
        return Err(ToolError::InvalidInput("invalid output cap".into()).into());
    }
    let cursor = cursor
        .as_deref()
        .map(|value| unwrap_cursor(handle, value))
        .transpose()?;
    let result = state
        .runner
        .session_output_for_manager(
            entry.manager,
            SessionOutputRequest {
                owner: actor.runner_owner(),
                session_key: Some(actor.principal_id.clone()),
                execution_key: handle.to_owned(),
                session_id: session,
                cursor,
                max_bytes: cap,
            },
        )
        .await;
    match result {
        Ok(chunk) => Ok(public_output(handle, chunk)?),
        Err(error) if missing(&error, session) => {
            note_missing(state, handle);
            if matches!(entry.state, ExecutionState::Terminal(_)) {
                Err(OutputError::Unavailable)
            } else {
                Err(retryable().into())
            }
        }
        Err(error)
            if matches!(
                error.to_string().as_str(),
                "invalid output cursor" | "future output cursor" | "invalid output cap"
            ) =>
        {
            Err(ToolError::InvalidInput("invalid output request".into()).into())
        }
        Err(_) => Err(retryable().into()),
    }
}

pub fn current(
    state: &AppState,
    actor: &ActorContext,
    handle: &str,
) -> Result<Descriptor, ToolError> {
    Ok(descriptor(handle, &lookup(state, actor, handle)?))
}

pub async fn input(
    state: &AppState,
    actor: &ActorContext,
    handle: &str,
    chars: String,
) -> Result<(), ToolError> {
    let started = Instant::now();
    match input_stages(state, actor, handle, chars).await {
        Ok(()) => Ok(()),
        Err(failure) => {
            warn_input_failure(failure.stage, started);
            Err(failure.error)
        }
    }
}

async fn input_stages(
    state: &AppState,
    actor: &ActorContext,
    handle: &str,
    chars: String,
) -> Result<(), InputFailure> {
    if chars.is_empty() {
        return Err(InputFailure::new(
            InputStage::Validation,
            ToolError::InvalidInput("empty session input".into()),
        ));
    }
    let gate = lookup(state, actor, handle)
        .map_err(|error| InputFailure::new(InputStage::Lookup, error))?
        .op_gate;
    let _guard = gate.lock().await;
    let entry = lookup(state, actor, handle)
        .map_err(|error| InputFailure::new(InputStage::Lookup, error))?;
    if !matches!(entry.state, ExecutionState::Running) {
        return Err(InputFailure::new(
            InputStage::State,
            ToolError::Forbidden("execution is not running".into()),
        ));
    }
    let session = entry
        .session
        .ok_or_else(retryable)
        .map_err(|error| InputFailure::new(InputStage::SessionMissing, error))?;
    let fence = admit_workspace_write(state, actor)
        .await
        .map_err(|error| InputFailure::new(InputStage::FenceAdmit, error))?;
    let input_hash = sha256_hex(&chars);
    let input_bytes = chars.len();
    let mut dispatched = DispatchedWriteGuard {
        registry: &state.executions,
        handle,
        armed: true,
        evidence: None,
    };
    state
        .runner
        .session_input_for_manager(
            entry.manager,
            SessionInputRequest {
                owner: actor.runner_owner(),
                session_key: Some(actor.principal_id.clone()),
                execution_key: handle.to_owned(),
                session_id: session,
                chars,
            },
        )
        .await
        .map_err(|_| InputFailure::new(InputStage::ManagerDispatch, retryable()))?;
    fence
        .commit()
        .await
        .map_err(|error| InputFailure::new(InputStage::FenceCommit, error))?;
    dispatched.armed = false;
    spawn_tool_audit(
        state,
        actor,
        "execution.input",
        serde_json::json!({"input_sha256": input_hash, "input_bytes": input_bytes, "status": "sent"}),
    );
    Ok(())
}

pub async fn cancel(
    state: &AppState,
    actor: &ActorContext,
    handle: &str,
) -> Result<Descriptor, ToolError> {
    let gate = lookup(state, actor, handle)?.op_gate;
    let _guard = gate.lock().await;
    let entry = lookup(state, actor, handle)?;
    if matches!(entry.state, ExecutionState::Terminal(_)) {
        return Ok(descriptor(handle, &entry));
    }
    if !matches!(entry.state, ExecutionState::Running) {
        return Err(retryable());
    }
    let session = entry.session.ok_or_else(retryable)?;
    let fence = admit_workspace_write(state, actor).await?;
    let mut dispatched = DispatchedWriteGuard {
        registry: &state.executions,
        handle,
        armed: true,
        evidence: None,
    };
    let status: RetainedSessionStatus = state
        .runner
        .session_cancel_for_manager(entry.manager, request(actor, handle, session))
        .await
        .map_err(|_| retryable())?;
    dispatched.evidence = Some(status.clone());
    if fence.commit().await.is_err() {
        return Err(retryable());
    }
    state
        .executions
        .update(handle, |entry| entry.observe(status));
    dispatched.armed = false;
    let result = descriptor(handle, &lookup(state, actor, handle)?);
    spawn_tool_audit(
        state,
        actor,
        "execution.cancel",
        serde_json::json!({"status": result.state}),
    );
    Ok(result)
}

pub async fn final_result(
    state: &AppState,
    actor: &ActorContext,
    handle: &str,
) -> Result<Descriptor, ToolError> {
    let result = status(state, actor, handle).await?;
    require_final(result)
}

pub(super) fn require_final(result: Descriptor) -> Result<Descriptor, ToolError> {
    if matches!(result.state.as_str(), "running" | "starting") {
        return Err(ToolError::Forbidden("execution is still running".into()));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn execution_start_guard_preserves_only_dispatched_uncertainty() {
        let registry = super::super::ExecutionRegistry::default();
        let owner = runner_protocol::RunnerOwner::InternalUser { user_id: 1 };
        let before = registry.reserve("p", 1, owner.clone(), 2).unwrap();
        drop(StartGuard {
            registry: &registry,
            handle: before.clone(),
            dispatched: false,
        });
        assert!(registry.get(&before, "p", 1, &owner).is_none());
        let after = registry.reserve("p", 1, owner.clone(), 2).unwrap();
        drop(StartGuard {
            registry: &registry,
            handle: after.clone(),
            dispatched: true,
        });
        assert!(matches!(
            registry.get(&after, "p", 1, &owner).unwrap().state,
            ExecutionState::Uncertain
        ));
    }

    #[test]
    fn execution_dispatched_write_future_drop_is_uncertain() {
        use std::{
            future::{Future, pending},
            task::{Context, Poll, Waker},
        };
        let registry = super::super::ExecutionRegistry::default();
        let owner = runner_protocol::RunnerOwner::InternalUser { user_id: 1 };
        let id = registry.reserve("p", 1, owner.clone(), 2).unwrap();
        registry.update(&id, |entry| entry.state = ExecutionState::Running);
        let mut cx = Context::from_waker(Waker::noop());
        let mut before = Box::pin(async {
            pending::<()>().await;
            let _g = DispatchedWriteGuard {
                registry: &registry,
                handle: &id,
                armed: true,
                evidence: None,
            };
        });
        assert!(matches!(before.as_mut().poll(&mut cx), Poll::Pending));
        drop(before);
        assert!(matches!(
            registry.get(&id, "p", 1, &owner).unwrap().state,
            ExecutionState::Running
        ));
        let mut after = Box::pin(async {
            let _g = DispatchedWriteGuard {
                registry: &registry,
                handle: &id,
                armed: true,
                evidence: None,
            };
            pending::<()>().await;
        });
        assert!(matches!(after.as_mut().poll(&mut cx), Poll::Pending));
        drop(after);
        let entry = registry.get(&id, "p", 1, &owner).unwrap();
        assert!(matches!(entry.state, ExecutionState::Uncertain));
        assert!(entry.terminal_at.is_some());
        assert!(!matches!(entry.state, ExecutionState::Running));
        let success = registry.reserve("p", 1, owner.clone(), 2).unwrap();
        registry.update(&success, |entry| entry.state = ExecutionState::Running);
        {
            let mut guard = DispatchedWriteGuard {
                registry: &registry,
                handle: &success,
                armed: true,
                evidence: None,
            };
            guard.armed = false;
        }
        assert!(matches!(
            registry.get(&success, "p", 1, &owner).unwrap().state,
            ExecutionState::Running
        ));
        let status = RetainedSessionStatus {
            state: runner_protocol::RetainedSessionState::Exited,
            exit_code: Some(0),
            output_complete: true,
            stdout_bytes: 0,
            stderr_bytes: 0,
            dropped_bytes: 0,
        };
        let mut cancel = Box::pin(async {
            let _g = DispatchedWriteGuard {
                registry: &registry,
                handle: &success,
                armed: true,
                evidence: Some(status.clone()),
            };
            pending::<()>().await;
        });
        assert!(matches!(cancel.as_mut().poll(&mut cx), Poll::Pending));
        drop(cancel);
        assert!(
            registry
                .get(&success, "p", 1, &owner)
                .unwrap()
                .uncertain_terminal
                .is_some()
        );
        registry.update(&success, |entry| {
            entry.state = ExecutionState::Terminal(status.clone())
        });
        drop(DispatchedWriteGuard {
            registry: &registry,
            handle: &success,
            armed: true,
            evidence: None,
        });
        assert!(matches!(
            registry.get(&success, "p", 1, &owner).unwrap().state,
            ExecutionState::Terminal(_)
        ));
    }

    #[test]
    fn execution_request_preserves_principal_and_separates_handles() {
        let owner = runner_protocol::RunnerOwner::InternalUser { user_id: 1 };
        let a = retained_request(owner.clone(), "principal", "execution-a", 1);
        let b = retained_request(owner, "principal", "execution-b", 1);
        assert_eq!(a.session_key.as_deref(), Some("principal"));
        assert_eq!(a.session_key, b.session_key);
        assert_ne!(a.execution_key, b.execution_key);
        assert_eq!(a.session_id, b.session_id);
    }

    #[test]
    fn execution_cursor_is_handle_bound() {
        let a = "11111111-1111-4111-8111-111111111111";
        let b = "22222222-2222-4222-8222-222222222222";
        let cursor = wrap_cursor(a, "v1:1:0:0").unwrap();
        assert_eq!(unwrap_cursor(a, &cursor).unwrap(), "v1:1:0:0");
        assert!(unwrap_cursor(b, &cursor).is_err());
        for bad in [
            format!("v2:{a}:eA"),
            format!("v1:{a}:!"),
            format!("v1:{a}:{}", "a".repeat(129)),
        ] {
            assert!(matches!(
                unwrap_cursor(a, &bad),
                Err(ToolError::InvalidInput(_))
            ));
        }
    }

    #[test]
    fn execution_missing_is_not_a_transient_error() {
        assert!(missing(&anyhow::anyhow!("unknown session_id 42"), 42));
        assert!(!missing(&anyhow::anyhow!("runner job timed out"), 42));
        assert!(!missing(&anyhow::anyhow!("unknown session_id 43"), 42));
    }
}
