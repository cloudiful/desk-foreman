//! Isolated database, HTTP and fencing helpers for the relay suite
//! (issue 749 P2b).
//!
//! Reuses the shared nonproduction URL helper and application/token helper
//! through `#[path]`, and adds the pieces this suite needs that the harness
//! does not already have: a raw HTTP caller that can send any method and
//! arbitrary headers, and a direct fence write so a case can put one
//! environment into an exact admitted state.

use std::time::Duration;

use desk_foreman::db::{queries::create_runner_manager, types::CreateRunnerManagerRequest};
use sqlx::PgPool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use super::helpers::{IsolatedDb, create_app_with_token};
use super::http::spawn_http;

/// A raw HTTP response, kept as text so binary relay bodies survive.
#[derive(Clone, Debug)]
pub struct HttpOutcome {
    pub status: u16,
    pub head: String,
    pub body: Vec<u8>,
}

impl HttpOutcome {
    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// Value of one response header, matched case-insensitively.
    pub fn header(&self, name: &str) -> Option<String> {
        self.head.lines().skip(1).find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case(name)
                .then(|| value.trim().to_string())
        })
    }
}

/// Split an `http://host:port` base into its parts.
pub fn split_base(base: &str) -> (String, u16) {
    let without_scheme = base.strip_prefix("http://").expect("http base URL");
    let (host, port) = without_scheme.rsplit_once(':').expect("host:port");
    (host.to_string(), port.parse().expect("port"))
}

/// Send one request with explicit method, path and headers.
///
/// The shared harness client only speaks JSON POST and GET, and this suite
/// needs a bodyless `DELETE`, an explicit `Content-Length`, and a caller
/// identity header on every authenticated call.
pub async fn call(
    base: &str,
    method: &str,
    path: &str,
    token: Option<&str>,
    headers: &[(String, String)],
    body: &[u8],
) -> HttpOutcome {
    let (host, port) = split_base(base);
    let mut stream = tokio::net::TcpStream::connect(format!("{host}:{port}"))
        .await
        .expect("connect test server");
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
        request.push_str("X-DF-External-User-Id: integration-test-user\r\n");
    }
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");
    if !body.is_empty() {
        stream.write_all(body).await.expect("write body");
    }
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), stream.read_to_end(&mut raw))
        .await
        .expect("relay response must not hang")
        .expect("read response");
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .unwrap_or(raw.len());
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let status: u16 = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("status code");
    let body = raw[split..].to_vec();
    let chunked = head.lines().skip(1).any(|line| {
        line.to_ascii_lowercase().starts_with("transfer-encoding:")
            && line.to_ascii_lowercase().contains("chunked")
    });
    HttpOutcome {
        status,
        head,
        body: if chunked { decode_chunked(&body) } else { body },
    }
}

/// Decode a chunked transfer body.
///
/// A relayed stream is almost always chunked, so a test client that skipped
/// this would read framing instead of the agent's bytes.
fn decode_chunked(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rest = raw;
    loop {
        let Some(line_end) = rest.windows(2).position(|window| window == b"\r\n") else {
            break;
        };
        let size = match std::str::from_utf8(&rest[..line_end])
            .ok()
            .and_then(|line| {
                usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16).ok()
            }) {
            Some(size) => size,
            None => break,
        };
        let start = line_end + 2;
        if size == 0 {
            break;
        }
        let end = (start + size).min(rest.len());
        out.extend_from_slice(&rest[start..end]);
        rest = &rest[(end + 2).min(rest.len())..];
    }
    out
}

/// Create an enabled runner manager and return its id plus bearer token.
pub async fn create_manager(pool: &PgPool, name: &str) -> (i64, String) {
    let (record, token) = create_runner_manager(
        pool,
        &CreateRunnerManagerRequest {
            name: name.to_string(),
            endpoint: format!("http://{name}.invalid"),
            access_token: None,
            image: "test-image".to_string(),
            network_enabled: false,
            max_output_bytes: 65_536,
            max_timeout_ms: 600_000,
            max_sessions: 2,
            pids_limit: 64,
            memory_limit: "256m".to_string(),
            cpu_limit: "1".to_string(),
            host_workspace_root: None,
        },
    )
    .await
    .expect("create runner manager");
    (
        record.runner_manager_id,
        token.expect("generated bearer token"),
    )
}

/// Put one environment into an exact admitted state.
///
/// Written directly because the admitted shape spans the invalidate
/// counter, the provision candidate and the immutable manager binding, and
/// the case needs each of them set independently.
pub async fn set_fence(
    pool: &PgPool,
    environment_id: Uuid,
    generation: i64,
    container_generation: Option<i64>,
    manager_id: Option<i64>,
    state: &str,
) {
    sqlx::query(
        "UPDATE execution_environments \
         SET generation = $2, container_generation = $3, assigned_manager_id = $4, state = $5 \
         WHERE environment_id = $1",
    )
    .bind(environment_id)
    .bind(generation)
    .bind(container_generation)
    .bind(manager_id)
    .bind(state)
    .execute(pool)
    .await
    .expect("write environment fence");
}

/// Create one environment row straight through the metadata query.
pub async fn create_environment(
    pool: &PgPool,
    application_id: i64,
    external_user_id: &str,
) -> Uuid {
    let outcome = desk_foreman::db::queries::ensure_execution_environment(
        pool,
        application_id,
        external_user_id,
        "opencode",
    )
    .await
    .expect("ensure environment");
    match outcome {
        desk_foreman::db::execution_environments::EnsureOutcome::Created(environment)
        | desk_foreman::db::execution_environments::EnsureOutcome::Existing(environment) => {
            environment.environment_id
        }
        desk_foreman::db::execution_environments::EnsureOutcome::RuntimeConflict { .. } => {
            panic!("ensure must not conflict for a fresh external user")
        }
    }
}

/// One admitted environment plus the identity used to reach it.
pub struct ServiceFixture {
    pub database: IsolatedDb,
    pub base: String,
    pub application_token: String,
    pub other_application_token: String,
    pub manager_id: i64,
    pub manager_token: String,
    pub environment_id: Uuid,
}

impl ServiceFixture {
    pub async fn setup() -> Self {
        let database = IsolatedDb::setup().await;
        let (application_id, application_token) =
            create_app_with_token(&database.pool, "service-owner").await;
        let (_other_id, other_application_token) =
            create_app_with_token(&database.pool, "service-other").await;
        let (manager_id, manager_token) = create_manager(&database.pool, "relay-manager").await;
        let environment_id = create_environment(&database.pool, application_id, "relay-user").await;
        set_fence(
            &database.pool,
            environment_id,
            4,
            Some(4),
            Some(manager_id),
            "ready",
        )
        .await;
        let state = super::helpers::test_state(database.pool.clone());
        let (base, _handle) = spawn_http(state).await;
        Self {
            database,
            base,
            application_token,
            other_application_token,
            manager_id,
            manager_token,
            environment_id,
        }
    }

    pub fn service_path(&self, suffix: &str) -> String {
        format!(
            "/api/internal/execution-environments/{}/service/{suffix}",
            self.environment_id
        )
    }

    pub async fn teardown(self) {
        self.database.teardown().await;
    }
}
