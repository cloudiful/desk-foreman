pub mod actor;
pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod lifecycle;
pub mod pathing;
pub mod policy;
pub mod runner;
pub mod shell;
pub mod tools;
pub mod workspace;

use std::sync::Arc;

use anyhow::Context;
use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, get_service},
};
use config::AppConfig;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use runner::{PullRunnerService, RunnerBroker, RunnerService};
use tools::DeskForemanService;
use tower_http::services::{ServeDir, ServeFile};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub runner: Arc<dyn RunnerService>,
    pub runner_broker: Arc<RunnerBroker>,
    pub db: sqlx::PgPool,
}

pub async fn run() -> anyhow::Result<()> {
    let config = Arc::new(AppConfig::from_env()?);
    let db = db::connect(&config).await?;
    db::migrate(&db).await?;
    db::bootstrap_admin(&db, &config).await?;
    let runner_broker = RunnerBroker::new(db.clone());
    runner_broker.spawn_liveness_monitor();
    let runner = PullRunnerService::new(Arc::clone(&runner_broker)) as Arc<dyn RunnerService>;

    let state = AppState {
        config: Arc::clone(&config),
        runner,
        runner_broker,
        db,
    };
    lifecycle::spawn_janitor(state.clone());

    let mcp_server_config = if config.mcp_allowed_hosts.is_empty() {
        StreamableHttpServerConfig::default().disable_allowed_hosts()
    } else {
        StreamableHttpServerConfig::default()
            .with_allowed_hosts(config.mcp_allowed_hosts.iter().cloned())
    };
    let mcp_service = StreamableHttpService::new(
        {
            let state = state.clone();
            move || Ok(DeskForemanService::new(state.clone()))
        },
        LocalSessionManager::default().into(),
        mcp_server_config,
    );

    let mut public = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .merge(api::router());
    if state.config.frontend_dist.is_dir() {
        let index = state.config.frontend_dist.join("index.html");
        public = public.fallback_service(get_service(
            ServeDir::new(&state.config.frontend_dist)
                .append_index_html_on_directories(true)
                .fallback(ServeFile::new(index)),
        ));
    }
    let protected = Router::new().nest_service("/mcp", mcp_service).route_layer(
        middleware::from_fn_with_state(state.clone(), bearer_auth_middleware),
    );
    let app = public.merge(protected).with_state(state);
    let listener = tokio::net::TcpListener::bind(&config.bind_addr)
        .await
        .context("failed to bind HTTP listener")?;

    axum::serve(listener, app)
        .await
        .context("HTTP server exited unexpectedly")
}

async fn healthz() -> &'static str {
    "ok"
}

async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    if !state.config.workspace_root.is_dir() {
        return (StatusCode::SERVICE_UNAVAILABLE, "workspace root missing");
    }
    if sqlx::query("SELECT 1").execute(&state.db).await.is_err() {
        return (StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    }
    match state.runner.list_sessions().await {
        Ok(_) => (StatusCode::OK, "ready"),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "runner-manager unavailable",
        ),
    }
}

async fn bearer_auth_middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut request: axum::extract::Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let Some(actor) = auth::identity::mcp_actor_from_bearer(&state, &headers, true)
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED)?
    else {
        return Err(StatusCode::UNAUTHORIZED);
    };
    request.extensions_mut().insert(actor);

    Ok(next.run(request).await)
}
