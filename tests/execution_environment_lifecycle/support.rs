//! Isolated database, manager and HTTP helpers for the lifecycle suite
//! (issue 742 P3).
//!
//! Reuses the shared nonproduction URL helper, application/token helper and
//! HTTP client from the execution-environment harness through `#[path]`, and
//! adds a reconnectable isolated database, an online manager, and the
//! poll/report helpers a mock runner-manager uses.

use std::time::Duration;

use desk_foreman::db::{queries::create_runner_manager, types::CreateRunnerManagerRequest};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

use super::helpers::test_database_url;
use super::http::{http_get, http_post};

fn replace_database_name(base_url: &str, db_name: &str) -> String {
    let (prefix, suffix) = match base_url.rfind('?') {
        Some(idx) => (&base_url[..idx], Some(&base_url[idx..])),
        None => (base_url, None),
    };
    let slash = prefix.rfind('/').expect("database URL must contain '/'");
    let head = &prefix[..slash + 1];
    match suffix {
        Some(query) => format!("{head}{db_name}{query}"),
        None => format!("{head}{db_name}"),
    }
}

/// Isolated per-test database with async cleanup and a reconnectable URL.
pub struct LifecycleDb {
    pub pool: PgPool,
    url: String,
    admin_pool: PgPool,
    db_name: String,
}

impl LifecycleDb {
    pub async fn setup() -> Self {
        let base_url = test_database_url();
        let admin_pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&base_url)
            .await
            .expect("connect to base test database");
        let db_name = format!("df_lifecycle_test_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE \"{db_name}\""
        )))
        .execute(&admin_pool)
        .await
        .expect("create isolated lifecycle test database");
        let url = replace_database_name(&base_url, &db_name);
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(Duration::from_secs(30))
            .connect(&url)
            .await
            .expect("connect to isolated lifecycle test database");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("run migrations on isolated lifecycle test database");
        Self {
            pool,
            url,
            admin_pool,
            db_name,
        }
    }

    /// A second pool against the same database, proving durable state is not
    /// tied to one connection.
    pub async fn reconnect(&self) -> PgPool {
        PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(30))
            .connect(&self.url)
            .await
            .expect("reconnect to isolated lifecycle test database")
    }

    pub async fn teardown(self) {
        let db_name = self.db_name.clone();
        self.pool.close().await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
        )))
        .execute(&self.admin_pool)
        .await
        .expect("drop isolated lifecycle test database");
        self.admin_pool.close().await;
    }
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

/// Create one environment row via the metadata `ensure` route.
pub async fn create_environment(base: &str, token: &str, external_user_id: &str) -> Uuid {
    let (status, body) = http_post(
        base,
        "/api/internal/execution-environments/ensure",
        Some(token),
        json!({"external_user_id": external_user_id, "runtime_kind": "opencode"}),
    )
    .await;
    assert_eq!(status, 200, "ensure must succeed, got {body}");
    Uuid::parse_str(body["environment_id"].as_str().expect("environment id")).expect("uuid")
}

/// One authenticated manager call registers liveness (`online`+recent), which
/// the internal selection query requires.
pub async fn bring_manager_online(base: &str, token: &str) {
    let (status, _) = http_get(base, "/api/internal/runner-manager/config", Some(token)).await;
    assert_eq!(status, 200, "manager config must succeed");
}

/// Poll the manager job route once. Returns a non-null durable job or panics.
pub async fn poll_job(base: &str, token: &str) -> Value {
    let (status, body) =
        http_get(base, "/api/internal/runner-manager/jobs/next", Some(token)).await;
    assert_eq!(status, 200, "job poll must succeed");
    assert!(
        !body.is_null(),
        "expected a claimable durable job, got {body}"
    );
    body
}

/// Report a successful job result and assert it was accepted.
pub async fn report_ok(base: &str, token: &str, job_id: &str, result: Value) {
    report(base, token, job_id, true, Some(result), None, 204).await;
}

/// Report a failed job result and assert it was accepted as an uncertain
/// blocker.
pub async fn report_failed(base: &str, token: &str, job_id: &str, error: &str) {
    report(base, token, job_id, false, None, Some(error), 204).await;
}

/// Report one job result and assert the HTTP status.
pub async fn report(
    base: &str,
    token: &str,
    job_id: &str,
    ok: bool,
    result: Option<Value>,
    error: Option<&str>,
    expected_status: u16,
) {
    let (status, body) = http_post(
        base,
        "/api/internal/runner-manager/jobs/result",
        Some(token),
        json!({"job_id": job_id, "ok": ok, "result": result, "error": error}),
    )
    .await;
    assert_eq!(
        status, expected_status,
        "unexpected result status for {job_id}: {body}"
    );
}

/// Ensure ack value for one owner.
pub fn ensure_ack(environment_id: Uuid, generation: i64) -> Value {
    json!({
        "owner": {"ExecutionEnvironment": {
            "environment_id": environment_id.to_string(),
            "generation": generation,
        }},
        "container_name": runner_protocol::RunnerOwner::ExecutionEnvironment {
            environment_id: environment_id.to_string(),
            generation,
        }
        .container_name(),
        "state": "running",
    })
}

/// Confirmed stop ack value for one owner candidate generation.
pub fn stop_ack(environment_id: Uuid, generation: i64, observed_state: &str) -> Value {
    json!({
        "owner": {"ExecutionEnvironment": {
            "environment_id": environment_id.to_string(),
            "generation": generation,
        }},
        "container_name": runner_protocol::RunnerOwner::ExecutionEnvironment {
            environment_id: environment_id.to_string(),
            generation,
        }
        .container_name(),
        "confirmed": true,
        "observed_state": observed_state,
    })
}

/// Set one manager's availability directly for offline-manager cases.
pub async fn set_manager_online(pool: &PgPool, manager_id: i64, online: bool) {
    if online {
        sqlx::query(
            "UPDATE runner_managers SET status = 'online', last_seen_at = NOW() \
             WHERE runner_manager_id = $1",
        )
        .bind(manager_id)
        .execute(pool)
        .await
        .expect("set manager online");
    } else {
        sqlx::query(
            "UPDATE runner_managers SET status = 'offline', last_seen_at = NULL \
             WHERE runner_manager_id = $1",
        )
        .bind(manager_id)
        .execute(pool)
        .await
        .expect("set manager offline");
    }
}
