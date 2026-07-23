pub mod actor;
pub mod api;
pub mod approval;
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
use runner::{HttpRunnerClient, RunnerService};
use server::{ServerConfig as HttpServerConfig, axum::Server as AxumServer, mcp};
use tools::DeskForemanService;
use tower_http::services::ServeDir;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub approval: Arc<approval::ApprovalService>,
    pub runner: Arc<dyn RunnerService>,
    pub db: sqlx::PgPool,
}

pub async fn run() -> anyhow::Result<()> {
    let config = Arc::new(AppConfig::from_env()?);
    let db = db::connect(&config).await?;
    db::migrate(&db).await?;
    db::bootstrap_admin(&db, &config).await?;
    let runner: Arc<dyn RunnerService> =
        HttpRunnerClient::new(config.runner_client.clone()) as Arc<dyn RunnerService>;

    let state = AppState {
        config: Arc::clone(&config),
        approval: Arc::new(approval::ApprovalService::from_env()),
        runner,
        db,
    };
    lifecycle::spawn_janitor(state.clone());

    let mcp_service = mcp::service(mcp::ServerConfig::new().with_service_path("/mcp"), {
        let state = state.clone();
        move || DeskForemanService::new(state.clone())
    })
    .context("failed to build MCP service")?;

    let mut public = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .merge(api::router());
    if state.config.frontend_dist.is_dir() {
        public = public.fallback_service(get_service(ServeDir::new(&state.config.frontend_dist)));
    }
    let protected = Router::new().nest_service("/mcp", mcp_service).route_layer(
        middleware::from_fn_with_state(state.clone(), bearer_auth_middleware),
    );
    let app = public.merge(protected);

    let config = HttpServerConfig::new()
        .with_listen_addr(config.bind_addr.clone())
        .with_app_data(state)
        .build()
        .context("invalid HTTP server config")?;

    AxumServer::new_with_state(config, app)
        .start()
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
