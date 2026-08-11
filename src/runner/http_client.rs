use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use reqwest::{Client, StatusCode};

use crate::{
    config::RunnerClientConfig,
    error::ErrorResponse,
    runner::{RunnerFuture, RunnerService},
};
use runner_protocol::{
    CancelSessionRequest, CommandOutput, ExecRequest, InputRequest, RunnerCommandRequest,
    RunnerSessionStatus, ShellToolOutput,
};

// Align with the tool-level shell timeout ceiling (MAX_TIMEOUT_MS, default
// 600s); a shorter total timeout would fail long-running compile/test commands
// that the tool layer explicitly allows.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub struct HttpRunnerClient {
    client: Client,
    base_url: String,
    auth_token: String,
}

impl HttpRunnerClient {
    pub fn new(config: RunnerClientConfig) -> Arc<Self> {
        Arc::new(Self {
            client: Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("reqwest client builder"),
            base_url: config.base_url.trim_end_matches('/').to_string(),
            auth_token: config.auth_token,
        })
    }

    async fn post_json<T, R>(&self, path: &str, payload: &T) -> anyhow::Result<R>
    where
        T: serde::Serialize + ?Sized,
        R: serde::de::DeserializeOwned,
    {
        let response = self
            .client
            .post(format!("{}{path}", self.base_url))
            .bearer_auth(&self.auth_token)
            .json(payload)
            .send()
            .await
            .with_context(|| format!("failed to call runner-manager {path}"))?;
        if response.status().is_success() {
            return response
                .json::<R>()
                .await
                .with_context(|| format!("failed to decode runner-manager response for {path}"));
        }

        let status = response.status();
        let message = response
            .json::<ErrorResponse>()
            .await
            .map(|body| body.error)
            .unwrap_or_else(|_| format!("runner-manager request failed with status {status}"));
        match status {
            StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND | StatusCode::FORBIDDEN => {
                anyhow::bail!(message)
            }
            _ => anyhow::bail!("runner-manager error: {message}"),
        }
    }
}

impl RunnerService for HttpRunnerClient {
    fn exec_shell<'a>(
        &'a self,
        request: ExecRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async move {
            self.post_json("/internal/runner/exec-shell", &request)
                .await
        })
    }

    fn write_stdin<'a>(
        &'a self,
        request: InputRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async move {
            self.post_json("/internal/runner/write-stdin", &request)
                .await
        })
    }

    fn run_command<'a>(
        &'a self,
        request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
        Box::pin(async move {
            self.post_json("/internal/runner/run-command", &request)
                .await
        })
    }

    fn cancel_session<'a>(
        &'a self,
        request: CancelSessionRequest,
    ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>> {
        Box::pin(async move {
            self.post_json("/internal/runner/cancel-session", &request)
                .await
        })
    }

    fn list_sessions<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>> {
        Box::pin(async move { self.post_json("/internal/runner/list-sessions", &()).await })
    }
}
