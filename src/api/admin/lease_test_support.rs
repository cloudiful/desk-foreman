//! Test-only fake runner for takeover cancellation tests.
//!
//! Extracted from `lease_helpers.rs` to keep each file under the 400-line
//! cap. Only compiled in test builds.

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use runner_protocol::{
    CancelSessionRequest, CommandOutput, ExecRequest, InputRequest, RunnerCommandRequest,
    RunnerOwner, RunnerSessionStatus, ShellToolOutput,
};
use sqlx::postgres::PgPoolOptions;
use tokio::sync::Mutex;

use crate::{
    AppState,
    config::AppConfig,
    policy::{ALL_SCOPES, ResourceLimits},
    runner::{RunnerBroker, RunnerFuture, RunnerService},
};

/// Records every `list_sessions` / `cancel_session` call so tests can
/// assert that the takeover helper only touches sessions owned by the
/// target binding and tolerates per-session failures without aborting.
#[derive(Default)]
pub(crate) struct FakeRunnerService {
    pub(crate) sessions: Mutex<Vec<RunnerSessionStatus>>,
}

impl FakeRunnerService {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl RunnerService for FakeRunnerService {
    fn exec_shell<'a>(
        &'a self,
        _request: ExecRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async move { anyhow::bail!("exec_shell not supported in fake") })
    }

    fn write_stdin<'a>(
        &'a self,
        _request: InputRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async move { anyhow::bail!("write_stdin not supported in fake") })
    }

    fn cancel_session<'a>(
        &'a self,
        request: CancelSessionRequest,
    ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>> {
        Box::pin(async move {
            let mut sessions = self.sessions.lock().await;
            let index = sessions
                .iter()
                .position(|session| session.session_id == request.session_id)
                .ok_or_else(|| anyhow::anyhow!("unknown session_id {}", request.session_id))?;
            let session = sessions.remove(index);
            if session.owner != request.owner {
                anyhow::bail!("session does not belong to current user");
            }
            Ok(RunnerSessionStatus {
                state: "cancelled".to_string(),
                ..session
            })
        })
    }

    fn list_sessions<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>> {
        Box::pin(async move { Ok(self.sessions.lock().await.clone()) })
    }

    fn run_command<'a>(
        &'a self,
        _request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
        Box::pin(async move { anyhow::bail!("run_command not supported in fake") })
    }

    fn cleanup_runner_owner<'a>(
        &'a self,
        owner: RunnerOwner,
    ) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            self.sessions
                .lock()
                .await
                .retain(|session| session.owner != owner);
            Ok(())
        })
    }
}

pub(crate) fn fake_state(runner: Arc<FakeRunnerService>) -> AppState {
    AppState {
        config: Arc::new(AppConfig {
            bind_addr: "127.0.0.1:0".to_string(),
            mcp_allowed_hosts: Vec::new(),
            workspace_root: PathBuf::from("/tmp"),
            default_shell: "bash".to_string(),
            session_idle_ttl: Duration::from_secs(60),
            max_output_bytes: 64 * 1024,
            server_scopes: ALL_SCOPES
                .iter()
                .map(|scope| (*scope).to_string())
                .collect(),
            server_limits: ResourceLimits {
                max_timeout_ms: Some(600_000),
                max_output_bytes: Some(64 * 1024),
                max_file_bytes: Some(50 * 1024 * 1024),
                max_sessions: None,
                network_enabled: true,
            },
            workspace_retention: Duration::from_secs(30 * 86_400),
            database_url: "postgres://example.invalid/test".to_string(),
            web_session_ttl: Duration::from_secs(3600),
            web_cookie_name: "desk_foreman_session".to_string(),
            web_cookie_secure: false,
            bootstrap_admin_login: None,
            bootstrap_admin_password: None,
            bootstrap_admin_display_name: None,
            bootstrap_admin_email: None,
            bootstrap_admin_timezone: "UTC".to_string(),
            frontend_dist: PathBuf::from("/tmp/frontend"),
            build_started_at: SystemTime::now(),
        }),
        runner,
        runner_broker: RunnerBroker::new(
            PgPoolOptions::new()
                .connect_lazy("postgres://example.invalid/test")
                .expect("lazy pool"),
        ),
        db: PgPoolOptions::new()
            .connect_lazy("postgres://example.invalid/test")
            .expect("lazy pool"),
    }
}

pub(crate) fn binding_session(session_id: u64, binding_id: i64) -> RunnerSessionStatus {
    RunnerSessionStatus {
        session_id,
        owner: RunnerOwner::WorkspaceBinding {
            workspace_binding_id: binding_id,
        },
        session_key: None,
        state: "running".to_string(),
        exit_code: None,
        timed_out: false,
        wall_time_seconds: 0.0,
    }
}
