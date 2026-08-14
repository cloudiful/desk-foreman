use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, bail};
use tokio::sync::RwLock;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunnerBackendKind {
    Direct,
    Docker,
}

impl RunnerBackendKind {
    pub fn parse(raw: &str) -> anyhow::Result<Self> {
        match raw {
            "direct" => Ok(Self::Direct),
            "docker" => Ok(Self::Docker),
            other => bail!("invalid RUNNER_BACKEND: {other}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RunnerManagerConfig {
    pub control_plane_url: Option<String>,
    pub bind_addr: String,
    pub auth_token: String,
    pub backend: RunnerBackendKind,
    pub workspace_root: PathBuf,
    pub host_workspace_root: PathBuf,
    pub image: String,
    pub workdir: String,
    pub network_enabled: bool,
    pub max_output_bytes: usize,
    pub max_timeout_ms: u64,
    pub max_sessions: usize,
    pub pids_limit: u64,
    pub memory_limit: String,
    pub cpu_limit: String,
    pub idle_ttl: Duration,
    pub docker_cli: String,
    pub docker_host: Option<String>,
    pub runtime_class: Option<String>,
}

pub type SharedRunnerManagerConfig = Arc<RwLock<RunnerManagerConfig>>;

impl RunnerManagerConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let workspace_root = workspace_root_from_env("WORKSPACE_ROOT")?;
        let host_workspace_root = env::var("RUNNER_HOST_WORKSPACE_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| workspace_root.clone());
        let backend = RunnerBackendKind::parse(
            &env::var("RUNNER_BACKEND").unwrap_or_else(|_| "docker".to_string()),
        )?;
        if matches!(backend, RunnerBackendKind::Direct) && !env_flag("RUNNER_ALLOW_DIRECT") {
            bail!("RUNNER_BACKEND=direct requires RUNNER_ALLOW_DIRECT=true");
        }
        Ok(Self {
            control_plane_url: env::var("DESK_FOREMAN_URL")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            bind_addr: env::var("RUNNER_MANAGER_BIND_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:3001".to_string()),
            auth_token: env::var("RUNNER_MANAGER_TOKEN")
                .context("RUNNER_MANAGER_TOKEN is required for runner-manager")?,
            backend,
            workspace_root,
            host_workspace_root,
            image: env::var("RUNNER_IMAGE")
                .unwrap_or_else(|_| "desk-foreman-workspace-runner:local".to_string()),
            workdir: env::var("RUNNER_WORKDIR").unwrap_or_else(|_| "/workspace".to_string()),
            network_enabled: env::var("RUNNER_NETWORK_ENABLED")
                .ok()
                .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE"))
                .unwrap_or(false),
            max_output_bytes: env::var("RUNNER_MAX_OUTPUT_BYTES")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(262_144),
            max_timeout_ms: env::var("RUNNER_MAX_TIMEOUT_MS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(600_000),
            max_sessions: env::var("RUNNER_MAX_SESSIONS")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| *value > 0)
                .unwrap_or(32),
            pids_limit: env::var("RUNNER_PIDS_LIMIT")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(256),
            memory_limit: env::var("RUNNER_MEMORY_LIMIT").unwrap_or_else(|_| "1g".to_string()),
            cpu_limit: env::var("RUNNER_CPU_LIMIT").unwrap_or_else(|_| "2".to_string()),
            idle_ttl: Duration::from_secs(
                env::var("RUNNER_IDLE_TTL_SEC")
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(1800),
            ),
            docker_cli: env::var("DOCKER_CLI").unwrap_or_else(|_| "docker".to_string()),
            docker_host: env::var("DOCKER_HOST").ok(),
            runtime_class: env::var("RUNNER_RUNTIME_CLASS")
                .ok()
                .filter(|value| !value.trim().is_empty()),
        })
    }

    pub async fn load_control_plane_config(&mut self) -> anyhow::Result<bool> {
        let Some(base_url) = self.control_plane_url.clone() else {
            return Ok(false);
        };
        let url = format!(
            "{}/api/internal/runner-manager/config",
            base_url.trim_end_matches('/')
        );
        let response = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(15))
            .build()
            .context("failed to build runner manager config client")?
            .get(&url)
            .bearer_auth(&self.auth_token)
            .send()
            .await
            .with_context(|| format!("failed to fetch runner manager config from {url}"))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            tracing::warn!(
                status = %response.status(),
                "desk-foreman has no runner manager config yet; using local defaults"
            );
            return Ok(false);
        }
        if !response.status().is_success() {
            anyhow::bail!(
                "desk-foreman rejected runner manager config request with status {}",
                response.status()
            );
        }
        let config: desk_foreman::db::types::RunnerManagerResponse = response
            .json()
            .await
            .context("failed to decode runner manager config")?;
        if !config.enabled {
            bail!("runner manager is disabled in desk-foreman");
        }
        self.image = config.image;
        self.network_enabled = config.network_enabled;
        self.max_output_bytes = usize::try_from(config.max_output_bytes)
            .context("runner manager max_output_bytes is out of range")?;
        self.max_timeout_ms = u64::try_from(config.max_timeout_ms)
            .context("runner manager max_timeout_ms is out of range")?;
        self.max_sessions = usize::try_from(config.max_sessions)
            .context("runner manager max_sessions is out of range")?;
        self.pids_limit = u64::try_from(config.pids_limit)
            .context("runner manager pids_limit is out of range")?;
        self.memory_limit = config.memory_limit;
        self.cpu_limit = config.cpu_limit;
        if let Some(host_workspace_root) = config
            .host_workspace_root
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            self.host_workspace_root = PathBuf::from(host_workspace_root);
        }
        Ok(true)
    }
}

fn workspace_root_from_env(name: &str) -> anyhow::Result<PathBuf> {
    let raw = env::var(name).unwrap_or_else(|_| "/workspace".into());
    canonical_workspace_root(name, Path::new(&raw))
}

fn env_flag(name: &str) -> bool {
    env::var(name)
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE"))
}

fn canonical_workspace_root(name: &str, path: &Path) -> anyhow::Result<PathBuf> {
    let workspace_root = path
        .canonicalize()
        .with_context(|| format!("{name} must exist and be accessible"))?;
    if !workspace_root.is_dir() {
        bail!("{name} must be a directory");
    }
    Ok(workspace_root)
}

#[cfg(test)]
mod tests {
    use super::RunnerBackendKind;

    #[test]
    fn parses_runner_backend_modes() {
        assert_eq!(
            RunnerBackendKind::parse("direct").expect("direct should parse"),
            RunnerBackendKind::Direct
        );
        assert_eq!(
            RunnerBackendKind::parse("docker").expect("docker should parse"),
            RunnerBackendKind::Docker
        );
        assert!(RunnerBackendKind::parse("nope").is_err());
    }
}
