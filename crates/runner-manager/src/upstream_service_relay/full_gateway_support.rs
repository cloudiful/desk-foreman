//! Full-gateway test support (issue 749 P2d, `#[cfg(test)]`).
//!
//! Isolated per-test PostgreSQL, real DF router state, and fixture setup
//! for the whole-wire proof. Each test creates its own database from
//! `DF_ENV_TEST_DATABASE_URL` and drops it afterwards; the shared database
//! is never reset. Missing URL fails instead of silently passing, so the
//! mandatory named gate never counts an skipped run as green.

#[cfg(test)]
pub(crate) mod support {
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
    use sqlx::PgPool;

    pub use super::super::full_gateway_db::db::IsolatedDb;
    use uuid::Uuid;

    /// Fake runner that never provisions: the relay path must succeed
    /// without touching it.
    pub struct NoopRunner;

    impl RunnerService for NoopRunner {
        fn exec_shell<'a>(
            &'a self,
            _request: ExecRequest,
        ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
            Box::pin(async move { anyhow::bail!("noop runner unsupported") })
        }
        fn write_stdin<'a>(
            &'a self,
            _request: InputRequest,
        ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
            Box::pin(async move { anyhow::bail!("noop runner unsupported") })
        }
        fn cancel_session<'a>(
            &'a self,
            _request: CancelSessionRequest,
        ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>> {
            Box::pin(async move { anyhow::bail!("noop runner unsupported") })
        }
        fn list_sessions<'a>(
            &'a self,
        ) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>> {
            Box::pin(async move { Ok(Vec::new()) })
        }
        fn run_command<'a>(
            &'a self,
            _request: RunnerCommandRequest,
        ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
            Box::pin(async move { anyhow::bail!("noop runner unsupported") })
        }
        fn cleanup_runner_owner<'a>(
            &'a self,
            _owner: RunnerOwner,
        ) -> RunnerFuture<'a, anyhow::Result<()>> {
            Box::pin(async move { Ok(()) })
        }
    }

    /// Build DF router state backed by the isolated pool.
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

    /// Spawn the real DF router on an ephemeral port.
    pub async fn spawn_df(state: AppState) -> (String, tokio::task::JoinHandle<()>) {
        let router = desk_foreman::api::router().with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("serve test router");
        });
        (format!("http://{addr}"), handle)
    }

    /// Create one application plus bearer token.
    pub async fn create_app(pool: &PgPool, name: &str) -> (i64, String) {
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

    /// Create an enabled runner manager plus bearer token.
    pub async fn create_manager(pool: &PgPool, name: &str) -> (i64, String) {
        use desk_foreman::db::types::CreateRunnerManagerRequest;
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
        (record.runner_manager_id, token.expect("bearer"))
    }

    /// Create one environment row and pin its fence to the admitted shape.
    pub async fn create_admitted_environment(
        pool: &PgPool,
        application_id: i64,
        manager_id: i64,
    ) -> Uuid {
        let outcome = desk_foreman::db::queries::ensure_execution_environment(
            pool,
            application_id,
            "gateway-user",
            "opencode",
        )
        .await
        .expect("ensure environment");
        let environment_id = match outcome {
            desk_foreman::db::execution_environments::EnsureOutcome::Created(env)
            | desk_foreman::db::execution_environments::EnsureOutcome::Existing(env) => {
                env.environment_id
            }
            desk_foreman::db::execution_environments::EnsureOutcome::RuntimeConflict { .. } => {
                panic!("ensure must not conflict for a fresh user")
            }
        };
        sqlx::query(
            "UPDATE execution_environments \
             SET generation = 4, container_generation = 4, assigned_manager_id = $2, state = 'ready' \
             WHERE environment_id = $1",
        )
        .bind(environment_id)
        .bind(manager_id)
        .execute(pool)
        .await
        .expect("write fence");
        environment_id
    }
}
