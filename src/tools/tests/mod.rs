mod behavior;
mod schema;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use chrono::Utc;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::Mutex;
use validator::Validate;

use crate::{
    AppState,
    actor::{ActorContext, ActorMode},
    config::AppConfig,
    db::types::UserRecord,
    policy::{ALL_SCOPES, AccessPolicy, ResourceLimits},
    runner::{RunnerFuture, RunnerService},
};
use runner_protocol::{
    CancelSessionRequest, CommandOutput, ExecRequest, InputRequest, RunnerCommandRequest,
    RunnerOwner, RunnerSessionStatus, ShellToolOutput,
};
use sqlx::postgres::PgPoolOptions;

#[derive(Default)]
struct FakeRunnerService {
    sessions: Mutex<HashMap<u64, RunnerOwner>>,
}

impl FakeRunnerService {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

impl RunnerService for FakeRunnerService {
    fn exec_shell<'a>(
        &'a self,
        request: ExecRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async move {
            if request.cmd == "cat" {
                self.sessions.lock().await.insert(1, request.owner);
                return Ok(ShellToolOutput {
                    wall_time_seconds: 0.01,
                    output: String::new(),
                    stdout: String::new(),
                    stderr: String::new(),
                    output_is_combined: false,
                    chunk_id: None,
                    exit_code: None,
                    session_id: Some(1),
                    original_token_count: None,
                    truncated: false,
                    has_more: false,
                    next_cursor: None,
                    stdout_bytes: 0,
                    stderr_bytes: 0,
                    timed_out: false,
                });
            }
            Ok(ShellToolOutput {
                wall_time_seconds: 0.01,
                output: if request.cmd == "printf 'hello'" {
                    "hello".to_string()
                } else {
                    String::new()
                },
                stdout: if request.cmd == "printf 'hello'" {
                    "hello".to_string()
                } else {
                    String::new()
                },
                stderr: String::new(),
                output_is_combined: false,
                chunk_id: None,
                exit_code: Some(0),
                session_id: None,
                original_token_count: None,
                truncated: false,
                has_more: false,
                next_cursor: None,
                stdout_bytes: if request.cmd == "printf 'hello'" {
                    5
                } else {
                    0
                },
                stderr_bytes: 0,
                timed_out: false,
            })
        })
    }

    fn write_stdin<'a>(
        &'a self,
        request: InputRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async move {
            let sessions = self.sessions.lock().await;
            let Some(owner) = sessions.get(&request.session_id) else {
                anyhow::bail!("unknown session_id {}", request.session_id);
            };
            if owner != &request.owner {
                anyhow::bail!("session does not belong to current user");
            }
            let stdout_bytes = request.chars.len();
            Ok(ShellToolOutput {
                wall_time_seconds: 0.01,
                output: request.chars,
                stdout: String::new(),
                stderr: String::new(),
                output_is_combined: false,
                chunk_id: None,
                exit_code: None,
                session_id: Some(request.session_id),
                original_token_count: None,
                truncated: false,
                has_more: false,
                next_cursor: None,
                stdout_bytes,
                stderr_bytes: 0,
                timed_out: false,
            })
        })
    }

    fn run_command<'a>(
        &'a self,
        request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
        Box::pin(async move {
            match request.program.as_str() {
                "rg" => {
                    let file = request.working_dir.join("search.txt");
                    if file.exists() {
                        Ok(CommandOutput {
                            output: "{\"type\":\"match\",\"data\":{\"path\":{\"text\":\"search.txt\"},\"lines\":{\"text\":\"beta\\n\"},\"line_number\":2}}\n".to_string(),
                            stdout: "{\"type\":\"match\",\"data\":{\"path\":{\"text\":\"search.txt\"},\"lines\":{\"text\":\"beta\\n\"},\"line_number\":2}}\n".to_string(),
                            exit_code: Some(0),
                            ..CommandOutput::default()
                        })
                    } else {
                        Ok(CommandOutput {
                            exit_code: Some(0),
                            ..CommandOutput::default()
                        })
                    }
                }
                other => anyhow::bail!("unsupported fake command: {other}"),
            }
        })
    }

    fn cancel_session<'a>(
        &'a self,
        request: CancelSessionRequest,
    ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>> {
        Box::pin(async move {
            let mut sessions = self.sessions.lock().await;
            let owner = sessions
                .remove(&request.session_id)
                .ok_or_else(|| anyhow::anyhow!("unknown session_id {}", request.session_id))?;
            if owner != request.owner {
                anyhow::bail!("session does not belong to current user");
            }
            Ok(RunnerSessionStatus {
                session_id: request.session_id,
                owner,
                session_key: request.session_key,
                state: "cancelled".to_string(),
                exit_code: None,
                timed_out: false,
                wall_time_seconds: 0.0,
            })
        })
    }

    fn list_sessions<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>> {
        Box::pin(async move {
            Ok(self
                .sessions
                .lock()
                .await
                .iter()
                .map(|(session_id, owner)| RunnerSessionStatus {
                    session_id: *session_id,
                    owner: owner.clone(),
                    session_key: None,
                    state: "running".to_string(),
                    exit_code: None,
                    timed_out: false,
                    wall_time_seconds: 0.0,
                })
                .collect())
        })
    }

    fn cleanup_runner_owner<'a>(
        &'a self,
        owner: RunnerOwner,
    ) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            self.sessions
                .lock()
                .await
                .retain(|_, session_owner| session_owner != &owner);
            Ok(())
        })
    }
}

fn app_state(root: PathBuf) -> AppState {
    app_state_with_runner(root, FakeRunnerService::new() as Arc<dyn RunnerService>)
}

fn app_state_with_runner(root: PathBuf, runner: Arc<dyn RunnerService>) -> AppState {
    AppState {
        config: Arc::new(AppConfig {
            bind_addr: "127.0.0.1:0".to_string(),
            mcp_allowed_hosts: Vec::new(),
            workspace_root: root.clone(),
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
            frontend_dist: root.join("frontend/dist"),
            build_started_at: SystemTime::now(),
        }),
        runner,
        runner_broker: crate::runner::RunnerBroker::new(
            PgPoolOptions::new()
                .connect_lazy("postgres://example.invalid/test")
                .expect("lazy pool"),
        ),
        db: PgPoolOptions::new()
            .connect_lazy("postgres://example.invalid/test")
            .expect("lazy pool"),
    }
}

pub(crate) struct StaticCommandRunner(pub CommandOutput);

impl RunnerService for StaticCommandRunner {
    fn exec_shell<'a>(
        &'a self,
        _request: ExecRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async move { anyhow::bail!("unsupported") })
    }

    fn write_stdin<'a>(
        &'a self,
        _request: InputRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async move { anyhow::bail!("unsupported") })
    }

    fn run_command<'a>(
        &'a self,
        _request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
        let output = self.0.clone();
        Box::pin(async move { Ok(output) })
    }

    fn cancel_session<'a>(
        &'a self,
        _request: CancelSessionRequest,
    ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>> {
        Box::pin(async move { anyhow::bail!("unsupported") })
    }

    fn list_sessions<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>> {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn cleanup_runner_owner<'a>(
        &'a self,
        _owner: RunnerOwner,
    ) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

fn parse_params<T: DeserializeOwned>(value: Value) -> Result<T, serde_json::Error> {
    serde_json::from_value(value)
}

fn parse_tool_params<T>(value: Value) -> Result<T, rmcp::ErrorData>
where
    T: DeserializeOwned + Validate,
{
    let object = value.as_object().cloned().expect("tool params object");
    crate::tools::common::parse_and_validate_tool_params(object)
}

fn top_level_keys(value: &Value) -> Vec<&str> {
    let mut keys = value
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys
}

fn test_actor(root: &Path, user_id: i64) -> ActorContext {
    let workspace_root = root.canonicalize().expect("canonical workspace root");
    ActorContext {
        mode: ActorMode::InternalUser,
        user: Some(UserRecord {
            user_id,
            login_name: format!("user-{user_id}"),
            password_hash: "hash".to_string(),
            display_name: format!("User {user_id}"),
            email: format!("user-{user_id}@example.com"),
            timezone: "UTC".to_string(),
            workspace_root: Some(workspace_root.to_string_lossy().to_string()),
            is_admin: user_id == 1,
            is_active: true,
            must_change_password: false,
            deleted_at: None,
            last_login_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }),
        application: None,
        workspace_binding: None,
        principal_id: format!("user:{user_id}"),
        external_user_id: None,
        workspace_binding_id: user_id,
        workspace_root,
        policy: AccessPolicy::new(
            ALL_SCOPES.iter().map(|scope| (*scope).to_string()),
            ResourceLimits::unrestricted(true),
        ),
        lease_owner: None,
    }
}
