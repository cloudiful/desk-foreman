//! Loopback Git Smart HTTP fixture plus DF gateway harness for the native
//! gateway integration test (issue 1180 P28).
//!
//! The fixture is a real `git http-backend` CGI wrapped in a minimal HTTP/1.1
//! server bound to loopback. It requires upstream Basic credentials, rejects
//! chunked bodies correctly, and supports upload-pack and receive-pack against a
//! temporary bare repository.

use std::{
    collections::HashMap,
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, SystemTime},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use desk_foreman::{
    AppState,
    config::{AppConfig, GitGatewayConfig},
    db::types::{CreateApplicationRequest, CreateApplicationTokenRequest},
    policy::{ALL_SCOPES, ResourceLimits},
    runner::{RunnerBroker, RunnerFuture, RunnerService},
};
use runner_protocol::{
    CancelSessionRequest, CommandOutput, ExecRequest, InputRequest, RunnerCommandRequest,
    RunnerOwner, RunnerSessionStatus, ShellToolOutput,
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

pub const UPSTREAM_USERNAME: &str = "upstream-user";
pub const UPSTREAM_PASSWORD: &str = "upstream-pass";
pub const EXTERNAL_USER_ID: &str = "fixture-user";

pub fn database_url() -> String {
    std::env::var("DF_GIT_NATIVE_TEST_DATABASE_URL").expect(
        "DF_GIT_NATIVE_TEST_DATABASE_URL is required for the native Git gateway test; \
         point it at the dedicated nonproduction issue database",
    )
}

/// Fake runner: gateway requests never touch it.
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

pub struct Fixture {
    pub pool: PgPool,
    pub application_id: i64,
    pub binding_id: i64,
    pub token: String,
    pub gateway_url: String,
    pub upstream_url: String,
    pub denied_url: String,
    pub redirect_url: String,
    pub bare_repo: PathBuf,
    pub scratch: PathBuf,
    backend: FixtureServer,
    _temp: tempfile::TempDir,
}

impl Fixture {
    pub async fn setup() -> Self {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url())
            .await
            .expect("connect to the native gateway test database");
        let temp = tempfile::tempdir().expect("tempdir");
        let scratch = temp.path().to_path_buf();

        let bare_repo = scratch.join("fixture").join("repo.git");
        std::fs::create_dir_all(bare_repo.parent().unwrap()).unwrap();
        run_ok("git", &["init", "--bare", path(&bare_repo)], &scratch, &[]);
        run_ok(
            "git",
            &["config", "http.receivepack", "true"],
            &bare_repo,
            &[],
        );
        seed_repository(&bare_repo, &scratch);

        let backend = FixtureServer::start(scratch.clone()).await;
        let upstream_url = format!("http://{}/fixture/repo.git", backend.addr);
        let denied_url = format!("http://{}/denied/repo.git", backend.addr);
        let redirect_url = format!("http://{}/redirect/repo.git", backend.addr);

        let application = desk_foreman::db::queries::create_application(
            &pool,
            &CreateApplicationRequest {
                name: format!("p28-native-{}", uuid::Uuid::new_v4()),
                workspace_template: None,
                default_shell: None,
                default_scopes: None,
                max_timeout_ms: None,
                max_output_bytes: None,
                max_file_bytes: None,
                max_sessions: None,
                network_enabled: None,
            },
        )
        .await
        .expect("create application");
        let (token, _) = desk_foreman::db::queries::create_application_token(
            &pool,
            &CreateApplicationTokenRequest {
                application_id: application.application_id,
                token_name: "native-fixture".to_string(),
                expires_at: None,
                scopes: None,
                max_timeout_ms: None,
                max_output_bytes: None,
                max_file_bytes: None,
                max_sessions: None,
                network_enabled: None,
            },
        )
        .await
        .expect("create application token");

        let root = scratch.join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        let binding = desk_foreman::db::queries::create_workspace_binding(
            &pool,
            application.application_id,
            EXTERNAL_USER_ID,
            "default",
            root.to_str().unwrap(),
            None,
            None,
        )
        .await
        .expect("create binding");

        for (alias, url) in [
            ("origin", &upstream_url),
            ("denied", &denied_url),
            ("redirect", &redirect_url),
        ] {
            desk_foreman::db::queries::create_workspace_git_upstream(
                &pool,
                binding.workspace_binding_id,
                desk_foreman::db::workspace_git_upstreams::NewWorkspaceGitUpstream {
                    remote_alias: alias,
                    target_url: url,
                    credential: Some((UPSTREAM_USERNAME, UPSTREAM_PASSWORD)),
                },
            )
            .await
            .expect("create upstream");
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gateway_addr = listener.local_addr().unwrap();
        let gateway_url = format!("http://{gateway_addr}");
        let state = test_state(pool.clone(), gateway_url.clone());
        tokio::spawn(async move {
            axum::serve(listener, desk_foreman::api::router().with_state(state))
                .await
                .expect("gateway server");
        });

        Self {
            pool,
            application_id: application.application_id,
            binding_id: binding.workspace_binding_id,
            token,
            gateway_url,
            upstream_url,
            denied_url,
            redirect_url,
            bare_repo,
            scratch,
            backend,
            _temp: temp,
        }
    }

    pub fn capability_url(&self) -> String {
        format!(
            "{}/api/internal/workspace-bindings/{}/git-capability",
            self.gateway_url, self.binding_id
        )
    }

    pub fn remote_url(&self, alias: &str) -> String {
        format!("{}/git/{}/{}", self.gateway_url, self.binding_id, alias)
    }

    pub async fn cleanup(self) {
        self.backend.abort();
        sqlx::query("DELETE FROM applications WHERE application_id = $1")
            .bind(self.application_id)
            .execute(&self.pool)
            .await
            .expect("delete fixture application");
        self.pool.close().await;
    }
}

fn test_state(pool: PgPool, gateway_url: String) -> AppState {
    let runner: Arc<dyn RunnerService> = Arc::new(NoopRunner);
    let broker_pool = pool.clone();
    AppState {
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
            git_gateway: GitGatewayConfig {
                public_base_url: Some(gateway_url),
                ..GitGatewayConfig::default()
            },
        }),
        git_capability_signing_key: desk_foreman::git_gateway::signing_key::from_bytes(&[7u8; 32])
            .expect("test signing key"),
        runner,
        runner_broker: RunnerBroker::new(broker_pool),
        db: pool,
    }
}

fn seed_repository(bare: &Path, scratch: &Path) {
    let work = scratch.join("seed-work");
    run_ok("git", &["init", path(&work)], scratch, &[]);
    run_ok(
        "git",
        &["config", "user.email", "fixture@example.com"],
        &work,
        &[],
    );
    run_ok("git", &["config", "user.name", "Fixture"], &work, &[]);
    std::fs::write(work.join("README.md"), "hello from the fixture\n").unwrap();
    run_ok("git", &["add", "README.md"], &work, &[]);
    run_ok(
        "git",
        &["commit", "-m", "initial fixture commit"],
        &work,
        &[],
    );
    run_ok("git", &["branch", "-M", "main"], &work, &[]);
    run_ok(
        "git",
        &["push", path(bare), "main:refs/heads/main"],
        &work,
        &[],
    );
}

pub fn run_git(args: &[&str], cwd: &Path, env: &[(&str, String)]) -> std::process::Output {
    let mut command = std::process::Command::new("git");
    command.args(args).current_dir(cwd);
    for (key, value) in env {
        command.env(*key, value);
    }
    command
        .output()
        .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"))
}

pub fn run_ok(program: &str, args: &[&str], cwd: &Path, env: &[(&str, String)]) {
    let mut command = std::process::Command::new(program);
    command.args(args).current_dir(cwd);
    for (key, value) in env {
        command.env(*key, value);
    }
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("failed to run {program} {args:?}: {error}"));
    assert!(
        output.status.success(),
        "{program} {args:?} failed: {}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
}

fn path(value: &Path) -> &str {
    value.to_str().unwrap()
}

struct FixtureServer {
    addr: SocketAddr,
    shutdown: tokio::sync::oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<()>,
}

impl FixtureServer {
    async fn start(root: PathBuf) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown, mut stop) = tokio::sync::oneshot::channel();
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut stop => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        let root = root.clone();
                        tokio::spawn(async move {
                            let _ = serve_connection(stream, &root).await;
                        });
                    }
                }
            }
        });
        Self {
            addr,
            shutdown,
            handle,
        }
    }

    fn abort(self) {
        let _ = self.shutdown.send(());
        self.handle.abort();
    }
}

async fn serve_connection(mut stream: TcpStream, root: &Path) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(index) = find(&buffer, b"\r\n\r\n") {
            break index;
        }
        if buffer.len() > 128 * 1024 {
            return Ok(());
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or("GET").to_string();
    let target = request_parts.next().unwrap_or("/").to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    let mut body = buffer[header_end + 4..].to_vec();
    if headers
        .get("transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"))
    {
        body = read_chunked(&mut stream, body).await?;
    } else if let Some(length) = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
    {
        while body.len() < length {
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read]);
        }
    }

    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    if path.starts_with("/redirect") {
        write_response(
            &mut stream,
            "302 Found",
            &[("Location", "http://127.0.0.1:9/elsewhere")],
            b"",
        )
        .await?;
        return Ok(());
    }
    if path.starts_with("/denied") {
        write_response(
            &mut stream,
            "401 Unauthorized",
            &[("WWW-Authenticate", "Basic realm=\"fixture\"")],
            b"denied\n",
        )
        .await?;
        return Ok(());
    }

    let expected = format!(
        "Basic {}",
        STANDARD.encode(format!("{UPSTREAM_USERNAME}:{UPSTREAM_PASSWORD}"))
    );
    if headers.get("authorization").map(String::as_str) != Some(expected.as_str()) {
        write_response(
            &mut stream,
            "401 Unauthorized",
            &[("WWW-Authenticate", "Basic realm=\"fixture\"")],
            b"authentication required\n",
        )
        .await?;
        return Ok(());
    }

    let output = if method == "GET" {
        run_cgi(root, &method, path, query, &headers, &[])
    } else {
        run_cgi(root, &method, path, query, &headers, &body)
    };
    match output {
        Ok((status, response_headers, response_body)) => {
            let header_refs = response_headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect::<Vec<_>>();
            write_response(&mut stream, &status, &header_refs, &response_body).await?;
        }
        Err(error) => {
            write_response(
                &mut stream,
                "500 Internal Server Error",
                &[],
                format!("fixture failure: {error}\n").as_bytes(),
            )
            .await?;
        }
    }
    Ok(())
}

async fn read_chunked(stream: &mut TcpStream, mut buffer: Vec<u8>) -> std::io::Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let line_end = loop {
            if let Some(index) = find(&buffer, b"\r\n") {
                break index;
            }
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                return Ok(body);
            }
            buffer.extend_from_slice(&chunk[..read]);
        };
        let line = String::from_utf8_lossy(&buffer[..line_end]).to_string();
        buffer.drain(..line_end + 2);
        let size =
            usize::from_str_radix(line.split(';').next().unwrap_or("0").trim(), 16).unwrap_or(0);
        if size == 0 {
            return Ok(body);
        }
        while buffer.len() < size + 2 {
            let read = stream.read(&mut chunk).await?;
            if read == 0 {
                return Ok(body);
            }
            buffer.extend_from_slice(&chunk[..read]);
        }
        body.extend_from_slice(&buffer[..size]);
        buffer.drain(..size + 2);
    }
}

fn run_cgi(
    root: &Path,
    method: &str,
    path: &str,
    query: &str,
    headers: &HashMap<String, String>,
    body: &[u8],
) -> std::io::Result<(String, Vec<(String, String)>, Vec<u8>)> {
    let mut command = std::process::Command::new("git");
    command
        .arg("http-backend")
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("PATH_INFO", path)
        .env("QUERY_STRING", query)
        .env("REQUEST_METHOD", method)
        .env("GATEWAY_INTERFACE", "CGI/1.1")
        .env("SERVER_PROTOCOL", "HTTP/1.1")
        .env("REMOTE_ADDR", "127.0.0.1")
        .env("REMOTE_USER", UPSTREAM_USERNAME)
        .env(
            "CONTENT_TYPE",
            headers
                .get("content-type")
                .cloned()
                .unwrap_or_else(|| "application/octet-stream".to_string()),
        )
        .env("CONTENT_LENGTH", body.len().to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(body);
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    let raw = output.stdout;
    let header_end = find(&raw, b"\r\n\r\n")
        .ok_or_else(|| std::io::Error::other("CGI response missing header terminator"))?;
    let head = String::from_utf8_lossy(&raw[..header_end]).to_string();
    let body = raw[header_end + 4..].to_vec();
    let mut status = "200 OK".to_string();
    let mut response_headers = Vec::new();
    for line in head.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("status") {
            status = value.trim().to_string();
        } else {
            response_headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    Ok((status, response_headers, body))
}

async fn write_response(
    stream: &mut TcpStream,
    status: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> std::io::Result<()> {
    let mut response = format!("HTTP/1.1 {status}\r\n");
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("content-length")
            || name.eq_ignore_ascii_case("transfer-encoding")
        {
            continue;
        }
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str(&format!("Content-Length: {}\r\n", body.len()));
    response.push_str("Connection: close\r\n\r\n");
    stream.write_all(response.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await?;
    Ok(())
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
