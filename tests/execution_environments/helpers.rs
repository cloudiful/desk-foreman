//! Shared helpers for execution-environment integration tests.
//!
//! Every test creates an isolated per-test PostgreSQL database from
//! `DF_ENV_TEST_DATABASE_URL` and drops it afterwards. The shared
//! database is never reset or truncated.

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use desk_foreman::{
    AppState,
    config::AppConfig,
    db::types::{CreateApplicationRequest, CreateApplicationTokenRequest},
    policy::{ALL_SCOPES, ResourceLimits},
    runner::{RunnerBroker, RunnerFuture, RunnerService},
};
use runner_protocol::{
    CancelSessionRequest, CommandOutput, ExecRequest, InputRequest, RunnerCommandRequest,
    RunnerOwner, RunnerSessionStatus, ShellToolOutput,
};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

/// Read the dedicated nonproduction test database URL.
///
/// Panics when missing so an explicit `cargo test --test
/// execution_environments` without the variable fails instead of
/// silently passing.
pub fn test_database_url() -> String {
    std::env::var("DF_ENV_TEST_DATABASE_URL").expect(
        "DF_ENV_TEST_DATABASE_URL is required for execution-environment integration tests; \
         set it to a dedicated nonproduction PostgreSQL URL (never production)",
    )
}

fn replace_database_name(base_url: &str, db_name: &str) -> String {
    let (prefix, suffix) = match base_url.rfind('?') {
        Some(idx) => (&base_url[..idx], Some(&base_url[idx..])),
        None => (base_url, None),
    };
    let slash = prefix.rfind('/').expect("database URL must contain '/'");
    let head = &prefix[..slash + 1];
    match suffix {
        Some(q) => format!("{head}{db_name}{q}"),
        None => format!("{head}{db_name}"),
    }
}

/// Isolated per-test database with async cleanup.
pub struct IsolatedDb {
    pub pool: PgPool,
    admin_pool: PgPool,
    db_name: String,
}

impl IsolatedDb {
    pub async fn setup() -> Self {
        let base_url = test_database_url();
        let admin_pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&base_url)
            .await
            .expect("connect to base test database");
        let db_name = format!(
            "df_env_test_{}",
            Uuid::new_v4().to_string().replace('-', "")
        );
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE \"{db_name}\""
        )))
        .execute(&admin_pool)
        .await
        .expect("create isolated test database");
        let db_url = replace_database_name(&base_url, &db_name);
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await
            .expect("connect to isolated test database");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("run migrations on isolated test database");
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
        .expect("drop isolated test database");
        self.admin_pool.close().await;
    }
}

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
pub fn test_state(pool: PgPool) -> AppState {
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
        }),
        runner,
        runner_broker: RunnerBroker::new(broker_pool),
        db: pool,
    }
}

/// Create one application plus bearer token for auth tests.
pub async fn create_app_with_token(pool: &PgPool, name: &str) -> (i64, String) {
    let app = desk_foreman::db::queries::create_application(
        pool,
        &CreateApplicationRequest {
            name: name.to_string(),
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
        pool,
        &CreateApplicationTokenRequest {
            application_id: app.application_id,
            token_name: format!("{name}-token"),
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
    (app.application_id, token)
}
