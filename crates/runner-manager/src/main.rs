mod config;
mod runtime;
mod server;

use crate::{
    config::RunnerManagerConfig,
    server::{RunnerManagerState, build_app, build_runner_service},
};
use anyhow::Context;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = RunnerManagerConfig::from_env()?;
    let runner = build_runner_service(&config).await?;
    let state = RunnerManagerState {
        auth_token: std::sync::Arc::<str>::from(config.auth_token.clone()),
        runner,
        max_output_bytes: config.max_output_bytes,
        max_timeout_ms: config.max_timeout_ms,
    };

    let app = build_app(state);

    let listener = tokio::net::TcpListener::bind(&config.bind_addr)
        .await
        .with_context(|| format!("failed to bind runner-manager on {}", config.bind_addr))?;
    axum::serve(listener, app)
        .await
        .context("runner-manager server exited unexpectedly")
}
