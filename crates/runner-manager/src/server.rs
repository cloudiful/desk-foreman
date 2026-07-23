use std::sync::Arc;

use anyhow::Context;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use desk_foreman::{db, error::ErrorResponse, runner::RunnerService};
use runner_protocol::{
    CancelSessionRequest, CommandOutput, ExecRequest, InputRequest, RunnerCommandRequest,
    RunnerSessionStatus, ShellToolOutput,
};

use crate::config::{RunnerBackendKind, RunnerManagerConfig};
use crate::runtime::{DirectRunnerBackend, DockerRunnerBackend, LocalRunnerService, RunnerBackend};

#[derive(Clone)]
pub(crate) struct RunnerManagerState {
    pub(crate) auth_token: Arc<str>,
    pub(crate) runner: Arc<dyn RunnerService>,
    pub(crate) max_output_bytes: usize,
    pub(crate) max_timeout_ms: u64,
}

pub(crate) fn build_app(state: RunnerManagerState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/internal/runner/exec-shell", post(exec_shell))
        .route("/internal/runner/write-stdin", post(write_stdin))
        .route("/internal/runner/cancel-session", post(cancel_session))
        .route("/internal/runner/list-sessions", post(list_sessions))
        .route("/internal/runner/run-command", post(run_command))
        .with_state(state.clone())
        .route_layer(middleware::from_fn_with_state(
            state,
            bearer_auth_middleware,
        ))
}

pub(crate) async fn build_runner_service(
    config: &RunnerManagerConfig,
) -> anyhow::Result<Arc<dyn RunnerService>> {
    let backend: Arc<dyn RunnerBackend> = match config.backend {
        RunnerBackendKind::Direct => DirectRunnerBackend::new() as Arc<dyn RunnerBackend>,
        RunnerBackendKind::Docker => {
            let database_url = config
                .database_url
                .as_deref()
                .context("DATABASE_URL is required when RUNNER_BACKEND=docker")?;
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(5)
                .connect(database_url)
                .await
                .context("failed to connect runner-manager to postgres")?;
            db::migrate(&pool).await?;
            DockerRunnerBackend::new(pool, config.clone()) as Arc<dyn RunnerBackend>
        }
    };

    let service = LocalRunnerService::new(
        backend,
        config.idle_ttl,
        config.max_output_bytes,
        config.max_sessions,
    );
    service.reconcile().await?;
    Ok(service as Arc<dyn RunnerService>)
}

async fn healthz() -> &'static str {
    "ok"
}

async fn exec_shell(
    State(state): State<RunnerManagerState>,
    Json(request): Json<ExecRequest>,
) -> Result<Json<ShellToolOutput>, AppError> {
    validate_limits(request.timeout_ms, request.max_output_bytes, &state)?;
    let output = state.runner.exec_shell(request).await?;
    Ok(Json(output))
}

async fn write_stdin(
    State(state): State<RunnerManagerState>,
    Json(request): Json<InputRequest>,
) -> Result<Json<ShellToolOutput>, AppError> {
    validate_limits(
        request.timeout_ms.or(request.yield_time_ms),
        request.max_output_bytes,
        &state,
    )?;
    let output = state.runner.write_stdin(request).await?;
    Ok(Json(output))
}

async fn cancel_session(
    State(state): State<RunnerManagerState>,
    Json(request): Json<CancelSessionRequest>,
) -> Result<Json<RunnerSessionStatus>, AppError> {
    Ok(Json(state.runner.cancel_session(request).await?))
}

async fn list_sessions(
    State(state): State<RunnerManagerState>,
    Json(_): Json<serde_json::Value>,
) -> Result<Json<Vec<RunnerSessionStatus>>, AppError> {
    Ok(Json(state.runner.list_sessions().await?))
}

async fn run_command(
    State(state): State<RunnerManagerState>,
    Json(request): Json<RunnerCommandRequest>,
) -> Result<Json<CommandOutput>, AppError> {
    validate_limits(request.timeout_ms, request.max_output_bytes, &state)?;
    let output = state.runner.run_command(request).await?;
    Ok(Json(output))
}

fn validate_limits(
    timeout_ms: Option<u64>,
    max_output_bytes: Option<usize>,
    state: &RunnerManagerState,
) -> Result<(), AppError> {
    if timeout_ms.is_some_and(|value| value > state.max_timeout_ms) {
        return Err(AppError::BadRequest(
            "command timeout exceeds runner-manager limit".to_string(),
        ));
    }
    if max_output_bytes.is_some_and(|value| value > state.max_output_bytes) {
        return Err(AppError::BadRequest(
            "command output exceeds runner-manager limit".to_string(),
        ));
    }
    Ok(())
}

async fn bearer_auth_middleware(
    State(state): State<RunnerManagerState>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: Next,
) -> Result<Response, StatusCode> {
    match bearer_token(&headers) {
        Some(token) if token == state.auth_token.as_ref() => Ok(next.run(request).await),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(axum::http::header::AUTHORIZATION)?;
    let raw = value.to_str().ok()?;
    raw.strip_prefix("Bearer ")
}

#[derive(Debug)]
enum AppError {
    BadRequest(String),
    Forbidden(String),
    NotFound(String),
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for AppError {
    fn from(value: anyhow::Error) -> Self {
        let message = value.to_string();
        if message == "session does not belong to current user" {
            return Self::NotFound("session not found".to_string());
        }
        if message.starts_with("unknown session_id ") {
            return Self::NotFound("session not found".to_string());
        }
        if matches!(
            message.as_str(),
            "absolute paths are not allowed"
                | "path must stay within workspace root"
                | "path escapes workspace root"
        ) || message.starts_with("failed to resolve ")
        {
            return Self::BadRequest(message);
        }
        if message.contains("does not belong to current user") {
            return Self::Forbidden(message);
        }
        Self::Internal(value)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error) = match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            Self::Forbidden(message) => (StatusCode::FORBIDDEN, message),
            Self::NotFound(message) => (StatusCode::NOT_FOUND, message),
            Self::Internal(error) => {
                tracing::error!(error = %error, "runner-manager request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
        };
        (status, Json(ErrorResponse { error })).into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use desk_foreman::runner::RunnerOwner;
    use reqwest::{Client, StatusCode};
    use runner_protocol::{
        CommandOutput, ExecRequest, InputRequest, RunnerCommandRequest, ShellToolOutput,
    };
    use tempfile::tempdir;

    use super::{RunnerManagerState, build_app};
    use crate::runtime::{DirectRunnerBackend, LocalRunnerService};

    async fn spawn_test_server() -> anyhow::Result<(String, tokio::task::JoinHandle<()>)> {
        let runner = LocalRunnerService::new(
            DirectRunnerBackend::new(),
            Duration::from_secs(60),
            262_144,
            4,
        );
        let state = RunnerManagerState {
            auth_token: Arc::<str>::from("test-token"),
            runner,
            max_output_bytes: 262_144,
            max_timeout_ms: 600_000,
        };
        let app = build_app(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let handle = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });
        Ok((format!("http://{addr}"), handle))
    }

    fn owner() -> RunnerOwner {
        RunnerOwner::InternalUser { user_id: 1 }
    }

    #[tokio::test]
    async fn exec_shell_endpoint_runs_command() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let workspace_root = temp.path().canonicalize()?;
        let (base_url, handle) = spawn_test_server().await?;
        let client = Client::new();

        let response = client
            .post(format!("{base_url}/internal/runner/exec-shell"))
            .bearer_auth("test-token")
            .json(&ExecRequest {
                owner: owner(),
                session_key: None,
                workspace_root,
                cmd: "printf 'hello'".to_string(),
                workdir: None,
                shell: "bash".to_string(),
                login: false,
                tty: false,
                timeout_ms: None,
                yield_time_ms: Some(50),
                max_output_tokens: None,
                max_output_bytes: None,
                network_enabled: false,
            })
            .send()
            .await?;

        assert_eq!(response.status(), StatusCode::OK);
        let output: ShellToolOutput = response.json().await?;
        assert_eq!(output.output, "hello");
        assert_eq!(output.stdout, "hello");
        assert!(output.stderr.is_empty());
        assert!(!output.output_is_combined);
        assert_eq!(output.exit_code, Some(0));
        handle.abort();
        Ok(())
    }

    #[tokio::test]
    async fn exec_shell_separates_stderr_and_marks_timeout() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let workspace_root = temp.path().canonicalize()?;
        let (base_url, handle) = spawn_test_server().await?;
        let client = Client::new();

        let response = client
            .post(format!("{base_url}/internal/runner/exec-shell"))
            .bearer_auth("test-token")
            .json(&ExecRequest {
                owner: owner(),
                session_key: None,
                workspace_root: workspace_root.clone(),
                cmd: "printf 'out'; printf 'err' >&2".to_string(),
                workdir: None,
                shell: "bash".to_string(),
                login: false,
                tty: false,
                timeout_ms: Some(1000),
                yield_time_ms: Some(50),
                max_output_tokens: None,
                max_output_bytes: None,
                network_enabled: false,
            })
            .send()
            .await?;
        let output: ShellToolOutput = response.json().await?;
        assert_eq!(output.stdout, "out");
        assert_eq!(output.stderr, "err");
        assert!(!output.output_is_combined);

        let response = client
            .post(format!("{base_url}/internal/runner/exec-shell"))
            .bearer_auth("test-token")
            .json(&ExecRequest {
                owner: owner(),
                session_key: None,
                workspace_root,
                cmd: "sleep 1".to_string(),
                workdir: None,
                shell: "bash".to_string(),
                login: false,
                tty: false,
                timeout_ms: Some(30),
                yield_time_ms: Some(100),
                max_output_tokens: None,
                max_output_bytes: None,
                network_enabled: false,
            })
            .send()
            .await?;
        let output: ShellToolOutput = response.json().await?;
        assert!(output.timed_out);
        handle.abort();
        Ok(())
    }

    #[tokio::test]
    async fn write_stdin_endpoint_writes_to_existing_session() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let workspace_root = temp.path().canonicalize()?;
        let (base_url, handle) = spawn_test_server().await?;
        let client = Client::new();

        let session = client
            .post(format!("{base_url}/internal/runner/exec-shell"))
            .bearer_auth("test-token")
            .json(&ExecRequest {
                owner: owner(),
                session_key: None,
                workspace_root,
                cmd: "cat".to_string(),
                workdir: None,
                shell: "bash".to_string(),
                login: false,
                tty: false,
                timeout_ms: None,
                yield_time_ms: Some(50),
                max_output_tokens: None,
                max_output_bytes: None,
                network_enabled: false,
            })
            .send()
            .await?
            .json::<ShellToolOutput>()
            .await?;

        let response = client
            .post(format!("{base_url}/internal/runner/write-stdin"))
            .bearer_auth("test-token")
            .json(&InputRequest {
                owner: owner(),
                session_key: None,
                session_id: session.session_id.expect("session id"),
                chars: "desk foreman\n".to_string(),
                yield_time_ms: Some(50),
                max_output_tokens: None,
                timeout_ms: None,
                max_output_bytes: None,
            })
            .send()
            .await?;

        assert_eq!(response.status(), StatusCode::OK);
        let output: ShellToolOutput = response.json().await?;
        assert!(output.output.contains("desk foreman"));
        handle.abort();
        Ok(())
    }

    #[tokio::test]
    async fn run_command_endpoint_runs_program() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let workspace_root = temp.path().canonicalize()?;
        let (base_url, handle) = spawn_test_server().await?;
        let client = Client::new();

        let response = client
            .post(format!("{base_url}/internal/runner/run-command"))
            .bearer_auth("test-token")
            .json(&RunnerCommandRequest {
                owner: owner(),
                workspace_root: workspace_root.clone(),
                working_dir: workspace_root,
                program: "bash".to_string(),
                args: vec!["-lc".to_string(), "pwd".to_string()],
                timeout_ms: None,
                max_output_bytes: None,
                network_enabled: false,
            })
            .send()
            .await?;

        assert_eq!(response.status(), StatusCode::OK);
        let output: CommandOutput = response.json().await?;
        assert!(
            output
                .output
                .trim()
                .ends_with(temp.path().to_string_lossy().as_ref())
        );
        handle.abort();
        Ok(())
    }

    #[tokio::test]
    async fn requests_require_bearer_auth() -> anyhow::Result<()> {
        let (base_url, handle) = spawn_test_server().await?;
        let client = Client::new();

        let response = client
            .post(format!("{base_url}/internal/runner/run-command"))
            .send()
            .await?;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        handle.abort();
        Ok(())
    }

    #[tokio::test]
    async fn command_output_is_truncated_at_request_limit() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let workspace_root = temp.path().canonicalize()?;
        let (base_url, handle) = spawn_test_server().await?;
        let response = Client::new()
            .post(format!("{base_url}/internal/runner/exec-shell"))
            .bearer_auth("test-token")
            .json(&ExecRequest {
                owner: owner(),
                session_key: None,
                workspace_root,
                cmd: "printf 123456".to_string(),
                workdir: None,
                shell: "bash".to_string(),
                login: false,
                tty: false,
                timeout_ms: Some(1000),
                yield_time_ms: Some(50),
                max_output_tokens: None,
                max_output_bytes: Some(3),
                network_enabled: false,
            })
            .send()
            .await?;
        let output: ShellToolOutput = response.json().await?;
        assert!(output.output.len() <= 3);
        assert!(output.truncated);
        handle.abort();
        Ok(())
    }

    #[tokio::test]
    async fn cancel_session_enforces_owner_and_is_not_found_after_removal() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let workspace_root = temp.path().canonicalize()?;
        let (base_url, handle) = spawn_test_server().await?;
        let client = Client::new();
        let session: ShellToolOutput = client
            .post(format!("{base_url}/internal/runner/exec-shell"))
            .bearer_auth("test-token")
            .json(&ExecRequest {
                owner: owner(),
                session_key: None,
                workspace_root,
                cmd: "cat".to_string(),
                workdir: None,
                shell: "bash".to_string(),
                login: false,
                tty: false,
                timeout_ms: None,
                yield_time_ms: Some(20),
                max_output_tokens: None,
                max_output_bytes: None,
                network_enabled: false,
            })
            .send()
            .await?
            .json()
            .await?;
        let id = session.session_id.expect("session id");
        let response = client
            .post(format!("{base_url}/internal/runner/cancel-session"))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "owner": { "InternalUser": { "user_id": 1 } },
                "session_key": null,
                "session_id": id
            }))
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .json::<runner_protocol::RunnerSessionStatus>()
                .await?
                .state,
            "cancelled"
        );
        let response = client
            .post(format!("{base_url}/internal/runner/cancel-session"))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "owner": { "InternalUser": { "user_id": 1 } },
                "session_key": null,
                "session_id": id
            }))
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        handle.abort();
        Ok(())
    }
}
