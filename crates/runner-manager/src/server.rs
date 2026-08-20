use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use desk_foreman::{error::ErrorResponse, runner::RunnerService};
use runner_protocol::{
    CancelSessionRequest, CommandOutput, ExecRequest, InputRequest, RunnerCommandRequest,
    RunnerSessionStatus, ShellToolOutput,
};

use crate::config::{RunnerBackendKind, SharedRunnerManagerConfig};
use crate::runtime::{
    DirectRunnerBackend, DockerRunnerBackend, LocalRunnerService, RunnerBackend,
    RunnerLifecycleReporter,
};

#[derive(Clone)]
pub(crate) struct RunnerManagerState {
    pub(crate) auth_token: Arc<str>,
    pub(crate) config: SharedRunnerManagerConfig,
    pub(crate) runner: Arc<dyn RunnerService>,
}

pub(crate) fn build_app(state: RunnerManagerState) -> Router {
    let protected = Router::new()
        .route("/internal/runner/exec-shell", post(exec_shell))
        .route("/internal/runner/write-stdin", post(write_stdin))
        .route("/internal/runner/cancel-session", post(cancel_session))
        .route("/internal/runner/list-sessions", post(list_sessions))
        .route("/internal/runner/run-command", post(run_command))
        .with_state(state.clone())
        .route_layer(middleware::from_fn_with_state(
            state,
            bearer_auth_middleware,
        ));

    Router::new()
        .route("/healthz", get(healthz))
        .merge(protected)
}

pub(crate) struct RunnerServiceHandles {
    pub(crate) service: Arc<dyn RunnerService>,
    pub(crate) docker_backend: Option<Arc<DockerRunnerBackend>>,
}

pub(crate) async fn build_runner_service(
    config: SharedRunnerManagerConfig,
    manager_id: String,
    reporter: Arc<RunnerLifecycleReporter>,
) -> anyhow::Result<RunnerServiceHandles> {
    let initial = config.read().await.clone();
    let (backend, docker_backend): (Arc<dyn RunnerBackend>, Option<Arc<DockerRunnerBackend>>) =
        match initial.backend {
            RunnerBackendKind::Direct => {
                (DirectRunnerBackend::new() as Arc<dyn RunnerBackend>, None)
            }
            RunnerBackendKind::Docker => {
                let docker = DockerRunnerBackend::new(Arc::clone(&config), manager_id, reporter);
                (docker.clone() as Arc<dyn RunnerBackend>, Some(docker))
            }
        };

    let service = LocalRunnerService::new(backend, config);
    service.reconcile().await?;
    Ok(RunnerServiceHandles {
        service: service as Arc<dyn RunnerService>,
        docker_backend,
    })
}

async fn healthz() -> &'static str {
    "ok"
}

async fn exec_shell(
    State(state): State<RunnerManagerState>,
    Json(mut request): Json<ExecRequest>,
) -> Result<Json<ShellToolOutput>, AppError> {
    let check_timeout = request.timeout_ms;
    apply_limits(
        &mut request.timeout_ms,
        &mut request.max_output_bytes,
        check_timeout,
        &state,
    )
    .await?;
    let output = state.runner.exec_shell(request).await?;
    Ok(Json(output))
}

async fn write_stdin(
    State(state): State<RunnerManagerState>,
    Json(mut request): Json<InputRequest>,
) -> Result<Json<ShellToolOutput>, AppError> {
    let check_timeout = request.timeout_ms.or(request.yield_time_ms);
    apply_limits(
        &mut request.timeout_ms,
        &mut request.max_output_bytes,
        check_timeout,
        &state,
    )
    .await?;
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
    Json(mut request): Json<RunnerCommandRequest>,
) -> Result<Json<CommandOutput>, AppError> {
    let check_timeout = request.timeout_ms;
    apply_limits(
        &mut request.timeout_ms,
        &mut request.max_output_bytes,
        check_timeout,
        &state,
    )
    .await?;
    let output = state.runner.run_command(request).await?;
    Ok(Json(output))
}

async fn apply_limits(
    timeout_ms: &mut Option<u64>,
    max_output_bytes: &mut Option<usize>,
    check_timeout: Option<u64>,
    state: &RunnerManagerState,
) -> Result<(), AppError> {
    let config = state.config.read().await;
    if check_timeout.is_some_and(|value| value > config.max_timeout_ms) {
        return Err(AppError::BadRequest(
            "command timeout exceeds runner-manager limit".to_string(),
        ));
    }
    if max_output_bytes.is_some_and(|value| value > config.max_output_bytes) {
        return Err(AppError::BadRequest(
            "command output exceeds runner-manager limit".to_string(),
        ));
    }
    *timeout_ms = Some(
        timeout_ms
            .unwrap_or(config.max_timeout_ms)
            .min(config.max_timeout_ms),
    );
    *max_output_bytes = Some(
        max_output_bytes
            .unwrap_or(config.max_output_bytes)
            .min(config.max_output_bytes),
    );
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
        let config = Arc::new(tokio::sync::RwLock::new(test_config()));
        let runner = LocalRunnerService::new(DirectRunnerBackend::new(), Arc::clone(&config));
        let state = RunnerManagerState {
            auth_token: Arc::<str>::from("test-token"),
            config,
            runner,
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

    fn test_config() -> crate::config::RunnerManagerConfig {
        crate::config::RunnerManagerConfig {
            control_plane_url: None,
            manager_id: "test-manager".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            auth_token: "test-token".to_string(),
            backend: crate::config::RunnerBackendKind::Direct,
            workspace_root: std::path::PathBuf::from("/tmp"),
            host_workspace_root: std::path::PathBuf::from("/tmp"),
            image: "test-image".to_string(),
            workdir: "/workspace".to_string(),
            network_enabled: false,
            max_output_bytes: 262_144,
            max_timeout_ms: 600_000,
            max_sessions: 4,
            pids_limit: 256,
            memory_limit: "1g".to_string(),
            cpu_limit: "2".to_string(),
            idle_ttl: Duration::from_secs(60),
            docker_cli: "docker".to_string(),
            docker_host: None,
            runtime_class: None,
        }
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
    async fn healthz_does_not_require_bearer_auth() -> anyhow::Result<()> {
        let (base_url, handle) = spawn_test_server().await?;
        let response = Client::new()
            .get(format!("{base_url}/healthz"))
            .send()
            .await?;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.text().await?, "ok");
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
