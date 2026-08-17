mod config;
mod runtime;
mod server;
mod upstream;

use crate::{
    config::{RunnerBackendKind, RunnerManagerConfig, SharedRunnerManagerConfig},
    server::{RunnerManagerState, build_app, build_runner_service},
};
use anyhow::Context;
use tokio::sync::RwLock;
use tokio::time::{Duration, sleep};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config: SharedRunnerManagerConfig =
        std::sync::Arc::new(RwLock::new(RunnerManagerConfig::from_env()?));
    if config.read().await.control_plane_url.is_some() {
        loop {
            match config.write().await.load_control_plane_config().await {
                Ok(true) => break,
                Ok(false) => {
                    tracing::info!("waiting for runner manager registration in desk-foreman");
                    sleep(Duration::from_secs(5)).await;
                }
                Err(error) => {
                    tracing::warn!(error = %error, "waiting for desk-foreman runner manager config");
                    sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }
    let initial = config.read().await.clone();
    if initial.backend == RunnerBackendKind::Docker
        && initial.host_workspace_root == initial.workspace_root
    {
        tracing::warn!(
            "host_workspace_root is not configured and defaults to WORKSPACE_ROOT; \
             docker bind mounts will use it as the host path, which is only correct \
             for bare-metal deployments. For containerized deployments set it on the \
             runner manager in the desk-foreman admin UI"
        );
    }
    let runner = build_runner_service(std::sync::Arc::clone(&config)).await?;
    let state = RunnerManagerState {
        auth_token: std::sync::Arc::<str>::from(config.read().await.auth_token.clone()),
        config: std::sync::Arc::clone(&config),
        runner: runner.clone(),
    };

    let app = build_app(state);

    if config.read().await.control_plane_url.is_some() {
        let upstream_config = std::sync::Arc::clone(&config);
        let upstream_runner = runner.clone();
        tokio::spawn(async move {
            if let Err(error) = upstream::run(upstream_config, upstream_runner).await {
                tracing::error!(error = %error, "runner upstream worker exited");
            }
        });
    }

    let bind_addr = config.read().await.bind_addr.clone();
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("failed to bind runner-manager on {bind_addr}"))?;
    axum::serve(listener, app)
        .await
        .context("runner-manager server exited unexpectedly")
}
