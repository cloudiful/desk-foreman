//! Isolated database and manager/seed helpers for the dispatch tests
//! (issue 742 P2).
//!
//! Reuses the shared nonproduction URL helper and the reusable HTTP client
//! from the execution-environment harness through `#[path]`, and adds only
//! what this suite needs: a reconnectable isolated database, a runner manager
//! with a known bearer token, and durable job seeding.

use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use desk_foreman::db::{
    queries::{EnqueueOutcome, EnsureOutcome, ensure_execution_environment},
    types::CreateRunnerManagerRequest,
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

use super::helpers::test_database_url;

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

/// Isolated per-test database with async cleanup.
pub struct DispatchDb {
    pub pool: PgPool,
    admin_pool: PgPool,
    db_name: String,
}

impl DispatchDb {
    pub async fn setup() -> Self {
        let base_url = test_database_url();
        let admin_pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&base_url)
            .await
            .expect("connect to base test database");
        let db_name = format!("df_dispatch_test_{}", Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE \"{db_name}\""
        )))
        .execute(&admin_pool)
        .await
        .expect("create isolated dispatch test database");
        let url = replace_database_name(&base_url, &db_name);
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(Duration::from_secs(30))
            .connect(&url)
            .await
            .expect("connect to isolated dispatch test database");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("run migrations on isolated dispatch test database");
        Self {
            pool,
            admin_pool,
            db_name,
        }
    }

    pub async fn teardown(self) {
        let db_name = self.db_name.clone();
        self.pool.close().await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
        )))
        .execute(&self.admin_pool)
        .await
        .expect("drop isolated dispatch test database");
        self.admin_pool.close().await;
    }
}

/// Create an enabled runner manager through the real creation path and return
/// its id plus the generated bearer token.
pub async fn create_manager_with_token(pool: &PgPool, name: &str) -> (i64, String) {
    let (record, token) = desk_foreman::db::queries::create_runner_manager(
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

/// Ensure one execution environment owned by `application_id`.
pub async fn create_environment(pool: &PgPool, application_id: i64, identity: &str) -> Uuid {
    match ensure_execution_environment(pool, application_id, identity, "opencode")
        .await
        .expect("ensure execution environment")
    {
        EnsureOutcome::Created(row) | EnsureOutcome::Existing(row) => row.environment_id,
        other => panic!("expected an environment row, got {other:?}"),
    }
}

/// Enqueue one durable job and return its stable row UUID.
pub async fn seed_job(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    manager_id: i64,
    kind: &str,
    generation: i64,
    token: &str,
    deadline: DateTime<Utc>,
) -> Uuid {
    match desk_foreman::db::queries::enqueue_execution_environment_job(
        pool,
        application_id,
        environment_id,
        manager_id,
        kind,
        generation,
        token,
        deadline,
    )
    .await
    .expect("enqueue environment job")
    {
        EnqueueOutcome::Created(row) => row.job_id,
        other => panic!("expected Created, got {other:?}"),
    }
}

/// Read the persisted state of one durable job.
pub async fn job_state(pool: &PgPool, job_id: Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM execution_environment_jobs WHERE job_id = $1")
        .bind(job_id)
        .fetch_one(pool)
        .await
        .expect("read job state")
}

/// Read the persisted terminal result of one durable job.
pub async fn job_result(pool: &PgPool, job_id: Uuid) -> Option<serde_json::Value> {
    sqlx::query_scalar("SELECT result FROM execution_environment_jobs WHERE job_id = $1")
        .bind(job_id)
        .fetch_one(pool)
        .await
        .expect("read job result")
}

/// Read the persisted redacted error summary of one durable job.
pub async fn job_error_summary(pool: &PgPool, job_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT error_summary FROM execution_environment_jobs WHERE job_id = $1")
        .bind(job_id)
        .fetch_one(pool)
        .await
        .expect("read job error summary")
}

/// A deadline comfortably in the future.
pub fn future_deadline() -> DateTime<Utc> {
    Utc::now() + ChronoDuration::seconds(600)
}

/// A deadline that has already passed.
pub fn past_deadline() -> DateTime<Utc> {
    Utc::now() - ChronoDuration::seconds(5)
}
