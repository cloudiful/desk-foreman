use std::{sync::Arc, time::Duration};

use anyhow::{Context, bail};
use desk_foreman::runner::RunnerService;
use reqwest::Client;
use runner_protocol::{
    CancelSessionRequest, ExecRequest, InputRequest, RUNNER_JOB_TIMEOUT_SECS, RunnerCommandRequest,
    RunnerJob, RunnerJobResult, RunnerOwner,
};
use serde_json::Value;
use tokio::time::Instant;
use tokio::time::sleep;

use crate::config::{RunnerManagerConfig, SharedRunnerManagerConfig};
use crate::runtime::session_gate::SessionGate;

const RETRY_DELAY: Duration = Duration::from_secs(5);
const CONFIG_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

pub async fn run(
    config: SharedRunnerManagerConfig,
    runner: Arc<dyn RunnerService>,
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
        let client = client.clone();
        let config = Arc::clone(&config);
        let gate = Arc::clone(&gate);
        let result_url = format!("{base_url}/api/internal/runner-manager/jobs/result");
        tokio::spawn(async move {
            let result = execute_job(&*runner, &config, &gate, job).await;
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

async fn execute_job(
    runner: &dyn RunnerService,
    config: &SharedRunnerManagerConfig,
    gate: &Arc<SessionGate>,
    job: RunnerJob,
) -> RunnerJobResult {
    let job_id = job.job_id.clone();
    match execute_job_inner(runner, config, gate, &job.kind, job.payload).await {
        Ok(value) => RunnerJobResult {
            job_id,
            ok: true,
            result: Some(value),
            error: None,
        },
        Err(error) => {
            tracing::error!(job_id, kind = %job.kind, error = %error, "runner job failed");
            RunnerJobResult {
                job_id,
                ok: false,
                result: None,
                error: Some(error.to_string()),
            }
        }
    }
}

async fn execute_job_inner(
    runner: &dyn RunnerService,
    config: &SharedRunnerManagerConfig,
    gate: &Arc<SessionGate>,
    kind: &str,
    payload: Value,
) -> anyhow::Result<Value> {
    match kind {
        "exec_shell" => {
            let mut request = serde_json::from_value::<ExecRequest>(payload)?;
            let _permit = gate.acquire(config).await;
            let limits = config.read().await.clone();
            apply_limits(&mut request, &limits)?;
            Ok(serde_json::to_value(runner.exec_shell(request).await?)?)
        }
        "write_stdin" => {
            let mut request = serde_json::from_value::<InputRequest>(payload)?;
            let limits = config.read().await.clone();
            apply_limits(&mut request, &limits)?;
            Ok(serde_json::to_value(runner.write_stdin(request).await?)?)
        }
        "cancel_session" => Ok(serde_json::to_value(
            runner
                .cancel_session(serde_json::from_value::<CancelSessionRequest>(payload)?)
                .await?,
        )?),
        "list_sessions" => Ok(serde_json::to_value(runner.list_sessions().await?)?),
        "run_command" => {
            let mut request = serde_json::from_value::<RunnerCommandRequest>(payload)?;
            let _permit = gate.acquire(config).await;
            let limits = config.read().await.clone();
            apply_limits(&mut request, &limits)?;
            Ok(serde_json::to_value(runner.run_command(request).await?)?)
        }
        "cleanup_runner_owner" => {
            let owner = serde_json::from_value::<RunnerOwner>(payload)?;
            runner.cleanup_runner_owner(owner).await?;
            Ok(Value::Null)
        }
        _ => bail!("unknown runner job kind: {kind}"),
    }
}

fn apply_limits<T>(request: &mut T, config: &RunnerManagerConfig) -> anyhow::Result<()>
where
    T: LimitableRequest,
{
    request.validate(config)?;
    request.apply_defaults(config);
    Ok(())
}

trait LimitableRequest {
    fn validate(&self, config: &RunnerManagerConfig) -> anyhow::Result<()>;
    fn apply_defaults(&mut self, config: &RunnerManagerConfig);
}

impl LimitableRequest for ExecRequest {
    fn validate(&self, config: &RunnerManagerConfig) -> anyhow::Result<()> {
        validate_limits(self.timeout_ms, self.max_output_bytes, config)
    }

    fn apply_defaults(&mut self, config: &RunnerManagerConfig) {
        self.timeout_ms = Some(
            self.timeout_ms
                .unwrap_or(config.max_timeout_ms)
                .min(config.max_timeout_ms),
        );
        self.max_output_bytes = Some(
            self.max_output_bytes
                .unwrap_or(config.max_output_bytes)
                .min(config.max_output_bytes),
        );
    }
}

impl LimitableRequest for RunnerCommandRequest {
    fn validate(&self, config: &RunnerManagerConfig) -> anyhow::Result<()> {
        validate_limits(self.timeout_ms, self.max_output_bytes, config)
    }

    fn apply_defaults(&mut self, config: &RunnerManagerConfig) {
        self.timeout_ms = Some(
            self.timeout_ms
                .unwrap_or(config.max_timeout_ms)
                .min(config.max_timeout_ms),
        );
        self.max_output_bytes = Some(
            self.max_output_bytes
                .unwrap_or(config.max_output_bytes)
                .min(config.max_output_bytes),
        );
    }
}

impl LimitableRequest for InputRequest {
    fn validate(&self, config: &RunnerManagerConfig) -> anyhow::Result<()> {
        validate_limits(
            self.timeout_ms.or(self.yield_time_ms),
            self.max_output_bytes,
            config,
        )
    }

    fn apply_defaults(&mut self, config: &RunnerManagerConfig) {
        self.max_output_bytes = Some(
            self.max_output_bytes
                .unwrap_or(config.max_output_bytes)
                .min(config.max_output_bytes),
        );
    }
}

fn validate_limits(
    timeout_ms: Option<u64>,
    max_output_bytes: Option<usize>,
    config: &RunnerManagerConfig,
) -> anyhow::Result<()> {
    if timeout_ms.is_some_and(|value| value > config.max_timeout_ms) {
        bail!("command timeout exceeds runner-manager limit");
    }
    if max_output_bytes.is_some_and(|value| value > config.max_output_bytes) {
        bail!("command output exceeds runner-manager limit");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc, time::Duration};

    use super::{SharedRunnerManagerConfig, apply_limits};
    use crate::config::{RunnerBackendKind, RunnerManagerConfig};
    use crate::runtime::session_gate::SessionGate;
    use runner_protocol::ExecRequest;
    use tokio::time::sleep;

    fn config() -> RunnerManagerConfig {
        RunnerManagerConfig {
            control_plane_url: None,
            manager_id: "test-manager".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            auth_token: "test-token".to_string(),
            backend: RunnerBackendKind::Direct,
            workspace_root: PathBuf::from("/tmp"),
            host_workspace_root: PathBuf::from("/tmp"),
            image: "test-image".to_string(),
            workdir: "/workspace".to_string(),
            network_enabled: false,
            max_output_bytes: 100,
            max_timeout_ms: 500,
            max_sessions: 1,
            pids_limit: 256,
            memory_limit: "1g".to_string(),
            cpu_limit: "2".to_string(),
            idle_ttl: Duration::from_secs(60),
            docker_cli: "docker".to_string(),
            docker_host: None,
            runtime_class: None,
        }
    }

    fn request() -> ExecRequest {
        ExecRequest {
            owner: runner_protocol::RunnerOwner::InternalUser { user_id: 1 },
            session_key: None,
            workspace_root: PathBuf::from("/tmp"),
            cmd: "true".to_string(),
            workdir: None,
            shell: "bash".to_string(),
            login: false,
            tty: false,
            timeout_ms: None,
            yield_time_ms: None,
            max_output_tokens: None,
            max_output_bytes: None,
            network_enabled: false,
        }
    }

    #[test]
    fn pull_limits_reject_overrides_and_default_missing_limits() {
        let limits = config();
        let mut bounded = request();
        apply_limits(&mut bounded, &limits).expect("missing limits should use manager defaults");
        assert_eq!(bounded.timeout_ms, Some(500));
        assert_eq!(bounded.max_output_bytes, Some(100));

        let mut oversized = request();
        oversized.timeout_ms = Some(501);
        assert!(apply_limits(&mut oversized, &limits).is_err());
    }

    #[tokio::test]
    async fn execution_gate_honors_current_session_limit() {
        let config: SharedRunnerManagerConfig = Arc::new(tokio::sync::RwLock::new(config()));
        let gate = SessionGate::new();
        let first = gate.acquire(&config).await;
        let waiting_gate = Arc::clone(&gate);
        let waiting_config = Arc::clone(&config);
        let waiting = tokio::spawn(async move { waiting_gate.acquire(&waiting_config).await });
        sleep(Duration::from_millis(20)).await;
        assert!(!waiting.is_finished());
        drop(first);
        waiting.await.expect("waiting task");
    }
}
