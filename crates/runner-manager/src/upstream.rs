//! Assigned-manager pull loop for the control plane (issue 742 P2).
//!
//! This module owns connection orchestration only: the poll cadence, the
//! periodic config refresh, the job decode, and submitting the result back.
//! What a job actually does lives in focused siblings — [`dispatch`] chooses
//! the executor, [`execute`] runs a legacy in-process job, and [`native_task`]
//! runs a durable preparation.

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use desk_foreman::runner::RunnerService;
use reqwest::Client;
use runner_protocol::{RUNNER_JOB_TIMEOUT_SECS, RunnerJob};
use tokio::time::Instant;
use tokio::time::sleep;

use crate::config::SharedRunnerManagerConfig;
use crate::runtime::RunnerBackend;
use crate::runtime::session_gate::SessionGate;

mod dispatch;
mod execute;
#[path = "upstream/native_task.rs"]
mod native_task;

#[cfg(test)]
#[path = "upstream/native_task_preparation.rs"]
mod native_task_preparation;

const RETRY_DELAY: Duration = Duration::from_secs(5);
const CONFIG_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

pub async fn run(
    config: SharedRunnerManagerConfig,
    runner: Arc<dyn RunnerService>,
    backend: Arc<dyn RunnerBackend>,
) -> anyhow::Result<()> {
    let Some(base_url) = config.read().await.control_plane_url.clone() else {
        tracing::info!("DESK_FOREMAN_URL is not configured; upstream pull worker is disabled");
        std::future::pending::<()>().await;
        unreachable!();
    };

    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(RUNNER_JOB_TIMEOUT_SECS))
        .build()
        .context("failed to build desk-foreman upstream client")?;
    let base_url = base_url.trim_end_matches('/').to_string();
    let gate = SessionGate::new();
    let mut last_config_refresh = Instant::now();

    loop {
        if last_config_refresh.elapsed() >= CONFIG_REFRESH_INTERVAL {
            if let Err(error) = config.write().await.load_control_plane_config().await {
                tracing::warn!(error = %error, "failed to refresh desk-foreman runner manager config");
            }
            last_config_refresh = Instant::now();
        }

        let response = client
            .get(format!("{base_url}/api/internal/runner-manager/jobs/next"))
            .bearer_auth(&config.read().await.auth_token)
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(error = %error, "failed to poll desk-foreman jobs");
                sleep(RETRY_DELAY).await;
                continue;
            }
        };
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            tracing::warn!("runner manager is not registered in desk-foreman");
            sleep(RETRY_DELAY).await;
            continue;
        }
        if !response.status().is_success() {
            tracing::warn!(status = %response.status(), "desk-foreman rejected job poll");
            sleep(RETRY_DELAY).await;
            continue;
        }
        let job: Option<RunnerJob> = match response.json().await {
            Ok(job) => job,
            Err(error) => {
                tracing::warn!(error = %error, "failed to decode desk-foreman job");
                sleep(RETRY_DELAY).await;
                continue;
            }
        };
        let Some(job) = job else { continue };
        let runner = Arc::clone(&runner);
        let backend = Arc::clone(&backend);
        let client = client.clone();
        let config = Arc::clone(&config);
        let gate = Arc::clone(&gate);
        let result_url = format!("{base_url}/api/internal/runner-manager/jobs/result");
        let poll_base = base_url.clone();
        tokio::spawn(async move {
            let result = dispatch::execute_job(
                &*runner, &*backend, &client, &poll_base, &config, &gate, job,
            )
            .await;
            let token = config.read().await.auth_token.clone();
            if let Err(error) = client
                .post(result_url)
                .bearer_auth(token)
                .json(&result)
                .send()
                .await
                .and_then(|response| response.error_for_status())
            {
                tracing::warn!(error = %error, "failed to submit runner job result");
            }
        });
    }
}
