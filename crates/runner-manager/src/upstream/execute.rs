//! In-process execution of a legacy runner job (issue 753 P2br).
//!
//! Every non-native job kind is decoded, bounded by the manager's own limits
//! and handed to the matching `RunnerService` method. The limit rules are the
//! authority on what a pulled request may ask for: a request without limits
//! gets the manager defaults, and one that exceeds them is refused.
//!
//! This is organization only: the branch set, the decode order, the gate
//! acquisition and the limit semantics are unchanged from the poll loop.

use std::sync::Arc;

use anyhow::bail;
use desk_foreman::runner::RunnerService;
use runner_protocol::{
    CancelSessionRequest, ExecRequest, InputRequest, RunnerCommandRequest, RunnerOwner,
};
use serde_json::Value;

use crate::config::{RunnerManagerConfig, SharedRunnerManagerConfig};
use crate::runtime::session_gate::SessionGate;

/// Execute one non-native job kind.
pub(super) async fn execute_job_inner(
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
        "ensure_execution_environment" => {
            let request = serde_json::from_value::<
                runner_protocol::EnsureExecutionEnvironmentRequest,
            >(payload)?;
            Ok(serde_json::to_value(
                runner.ensure_execution_environment(request).await?,
            )?)
        }
        "stop_execution_environment" => {
            let request = serde_json::from_value::<runner_protocol::StopExecutionEnvironmentRequest>(
                payload,
            )?;
            Ok(serde_json::to_value(
                runner.stop_execution_environment(request).await?,
            )?)
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
            native_task_root: None,
            native_task_host_root: None,
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
