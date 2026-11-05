use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, bail};
use tokio::sync::RwLock;

#[path = "config/native_task_paths.rs"]
pub(crate) mod native_task_paths;

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
    pub manager_id: String,
    pub bind_addr: String,
    pub auth_token: String,
    pub backend: RunnerBackendKind,
    pub workspace_root: PathBuf,
    pub host_workspace_root: PathBuf,
    pub image: String,
    /// Optional dedicated image for the trusted Git helper. Missing or
    /// malformed values are classified by Git availability, never fatal.
    pub git_image: Option<String>,
    /// Optional manager-private root for trusted Git operations.
    pub git_private_root: Option<PathBuf>,
    /// Optional daemon-visible counterpart of `git_private_root`.
    pub git_host_private_root: Option<PathBuf>,
    /// Optional dedicated Docker network for credentialed clone fetch steps.
    pub git_network: Option<String>,
    /// Optional exact broker authority (`host[:port]`) pinned for clone fetches.
    pub git_broker_host: Option<String>,
    pub git_max_fetch_volume_bytes: Option<u64>,
    pub git_max_bundle_bytes: Option<u64>,
    pub git_max_logical_object_bytes: Option<u64>,
    pub git_max_checkout_bytes: Option<u64>,
    pub git_max_tree_entries: Option<u64>,
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
    /// Explicit trusted root for native-task checkouts.
    ///
    /// Explicit local root for native-task checkouts. `None` disables
    /// preparation; it never falls back to a workspace root.
    pub native_task_root: Option<PathBuf>,
    /// Optional daemon-visible root used only by native-task bind mounts.
    /// It is never inferred from either local root.
    pub native_task_host_root: Option<PathBuf>,
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
            manager_id: env::var("RUNNER_MANAGER_ID")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| format!("standalone:{}", workspace_root.display())),
            bind_addr: env::var("RUNNER_MANAGER_BIND_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:3001".to_string()),
            auth_token: env::var("RUNNER_MANAGER_TOKEN")
                .context("RUNNER_MANAGER_TOKEN is required for runner-manager")?,
            backend,
            workspace_root,
            host_workspace_root,
            image: env::var("RUNNER_IMAGE")
                .unwrap_or_else(|_| "desk-foreman-workspace-runner:local".to_string()),
            git_image: optional_env_value(env::var("RUNNER_GIT_IMAGE").ok()),
            git_private_root: optional_env_value(env::var("RUNNER_GIT_PRIVATE_ROOT").ok())
                .map(PathBuf::from),
            git_host_private_root: optional_env_value(
                env::var("RUNNER_GIT_HOST_PRIVATE_ROOT").ok(),
            )
            .map(PathBuf::from),
            git_network: optional_env_value(env::var("RUNNER_GIT_NETWORK").ok()),
            git_broker_host: optional_env_value(env::var("RUNNER_GIT_BROKER_HOST").ok()),
            git_max_fetch_volume_bytes: env::var("RUNNER_GIT_MAX_FETCH_VOLUME_BYTES")
                .ok()
                .and_then(|value| value.parse().ok()),
            git_max_bundle_bytes: env::var("RUNNER_GIT_MAX_BUNDLE_BYTES")
                .ok()
                .and_then(|value| value.parse().ok()),
            git_max_logical_object_bytes: env::var("RUNNER_GIT_MAX_LOGICAL_OBJECT_BYTES")
                .ok()
                .and_then(|value| value.parse().ok()),
            git_max_checkout_bytes: env::var("RUNNER_GIT_MAX_CHECKOUT_BYTES")
                .ok()
                .and_then(|value| value.parse().ok()),
            git_max_tree_entries: env::var("RUNNER_GIT_MAX_TREE_ENTRIES")
                .ok()
                .and_then(|value| value.parse().ok()),
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
            native_task_root: native_task_paths::native_task_root_from_env()?,
            native_task_host_root: native_task_paths::native_task_host_root_from_env()?,
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
        self.manager_id = config.runner_manager_id.to_string();
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
            self.host_workspace_root = PathBuf::from(host_workspace_root.trim());
        }
        Ok(true)
    }
}

fn optional_env_value(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
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
    use super::{RunnerBackendKind, optional_env_value};

    #[test]
    fn optional_git_values_drop_blank_but_retain_invalid_nonempty() {
        assert_eq!(optional_env_value(None), None);
        assert_eq!(optional_env_value(Some(String::new())), None);
        assert_eq!(optional_env_value(Some(" \t\n".to_owned())), None);
        assert_eq!(
            optional_env_value(Some("relative/root".to_owned())),
            Some("relative/root".to_owned())
        );
        assert_eq!(
            optional_env_value(Some(" runner@sha256:abc ".to_owned())),
            Some(" runner@sha256:abc ".to_owned())
        );
    }

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
