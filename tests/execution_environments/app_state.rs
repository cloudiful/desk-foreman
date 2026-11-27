//! `AppState` fixture for HTTP-based execution-environment integration
//! suites, backed by an isolated pool and a no-op runner.

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use desk_foreman::{
    AppState,
    config::AppConfig,
    db::task_runtime::TaskRuntime,
    policy::{ALL_SCOPES, ResourceLimits},
    runner::{RunnerBroker, RunnerFuture, RunnerService},
};
use runner_protocol::{
    CancelSessionRequest, CommandOutput, ExecRequest, InputRequest, RunnerCommandRequest,
    RunnerOwner, RunnerSessionStatus, ShellToolOutput,
};
use sqlx::PgPool;

/// Fake runner that never provisions: every execution-environment route
/// must succeed without touching it.
pub struct NoopRunner;

impl RunnerService for NoopRunner {
    fn exec_shell<'a>(
        &'a self,
        _request: ExecRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async move { anyhow::bail!("noop runner: exec_shell unsupported") })
    }

    fn write_stdin<'a>(
        &'a self,
        _request: InputRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async move { anyhow::bail!("noop runner: write_stdin unsupported") })
    }

    fn cancel_session<'a>(
        &'a self,
        _request: CancelSessionRequest,
    ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>> {
        Box::pin(async move { anyhow::bail!("noop runner: cancel_session unsupported") })
    }

    fn list_sessions<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>> {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn run_command<'a>(
        &'a self,
        _request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
        Box::pin(async move { anyhow::bail!("noop runner: run_command unsupported") })
    }

    fn cleanup_runner_owner<'a>(
        &'a self,
        _owner: RunnerOwner,
    ) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

/// Build an `AppState` for HTTP tests backed by the isolated pool.
pub async fn test_state(pool: PgPool) -> AppState {
    let runner: Arc<dyn RunnerService> = Arc::new(NoopRunner);
    let broker_pool = pool.clone();
    AppState {
        executions: Arc::new(desk_foreman::tools::execution::ExecutionRegistry::default()),
        config: Arc::new(AppConfig {
            bind_addr: "127.0.0.1:0".to_string(),
            mcp_allowed_hosts: Vec::new(),
            workspace_root: PathBuf::from("/tmp"),
            default_shell: "bash".to_string(),
            session_idle_ttl: Duration::from_secs(60),
            max_output_bytes: 64 * 1024,
            server_scopes: ALL_SCOPES.iter().map(|s| s.to_string()).collect(),
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
            git_gateway: Default::default(),
            credential_protector: None,
        }),
        git_capability_signing_key: desk_foreman::git_gateway::signing_key::from_bytes(&[7u8; 32])
            .expect("test signing key"),
        runner,
        runner_broker: RunnerBroker::new(broker_pool),
        db: pool.clone(),
        task_runtime: Arc::new(TaskRuntime::new(pool)),
        authz: Arc::new(
            desk_foreman::authz::build()
                .await
                .expect("test authorization engine"),
        ),
    }
}
