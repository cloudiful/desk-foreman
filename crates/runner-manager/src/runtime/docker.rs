use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Instant,
};

use anyhow::Context;
use runner_protocol::{CommandOutput, RunnerOwner};
use tokio::process::Command;

use crate::config::{RunnerManagerConfig, SharedRunnerManagerConfig};

use super::{RunnerLifecycleReporter, docker_command::ensure_docker_command_succeeded};

pub struct DockerRunnerBackend {
    pub(super) config: SharedRunnerManagerConfig,
    pub(super) manager_id: String,
    pub(super) reporter: Arc<RunnerLifecycleReporter>,
    pub(super) observed: Mutex<HashMap<String, runner_protocol::RunnerLifecycleEvent>>,
    /// Serializes container create/start reconciliation per owner so concurrent
    /// first requests cannot race on `docker run --name <same-name>`.
    pub(super) runner_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    pub(super) last_active: Mutex<HashMap<String, Instant>>,
    pub(super) active_ops: Mutex<HashMap<String, usize>>,
}

impl DockerRunnerBackend {
    pub fn new(
        config: SharedRunnerManagerConfig,
        manager_id: String,
        reporter: Arc<RunnerLifecycleReporter>,
    ) -> Arc<Self> {
        Arc::new(Self {
            config,
            manager_id,
            reporter,
            observed: Mutex::new(HashMap::new()),
            runner_locks: Mutex::new(HashMap::new()),
            last_active: Mutex::new(HashMap::new()),
            active_ops: Mutex::new(HashMap::new()),
        })
    }

    pub(super) fn runner_lock(&self, owner: &RunnerOwner) -> Arc<tokio::sync::Mutex<()>> {
        let key = owner.stable_key().to_string();
        let mut locks = self.runner_locks.lock().expect("runner lock map poisoned");
        locks
            .entry(key)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    pub(super) async fn ensure_runner(
        &self,
        owner: &RunnerOwner,
        workspace_root: &Path,
        requested_network_enabled: bool,
    ) -> anyhow::Result<String> {
        let config = self.config.read().await.clone();
        let lock = self.runner_lock(owner);
        let _guard = lock.lock().await;
        let network_enabled = requested_network_enabled && config.network_enabled;
        let container_name = owner.container_name();
        let host_workspace_root = Self::host_workspace_root(&config, workspace_root)?;
        let mut inspected = self.inspect_container_metadata(&container_name).await?;
        if let Some(metadata) = &inspected {
            if metadata.manager_id.as_deref() == Some(self.manager_id.as_str())
                && metadata.owner_key.as_deref() == Some(owner.stable_key().as_str())
            {
                // This container belongs to the current runner manager.
            } else if metadata.manager_id.is_none()
                && metadata.owner_key.as_deref() == Some(owner.stable_key().as_str())
                && super::docker_lifecycle::legacy_workspace_mount_matches(
                    &config,
                    metadata.workspace_source.as_deref(),
                )
            {
                self.stop_and_remove(&container_name).await?;
                inspected = None;
            } else {
                anyhow::bail!(
                    "runner container {container_name} has a different runner-manager identity; set RUNNER_MANAGER_ID consistently or remove the stale container manually"
                );
            }
        }
        if inspected.is_none() {
            self.create_runner(
                owner,
                workspace_root,
                &host_workspace_root,
                &container_name,
                network_enabled,
                &config,
            )
            .await?;
            inspected = self.inspect_container_metadata(&container_name).await?;
        }

        let mut inspected = inspected.with_context(|| {
            format!("runner container {container_name} could not be inspected after creation")
        })?;

        if inspected.status != "running" {
            self.docker_status(["start", container_name.as_str()])
                .await?;
            inspected = self
                .inspect_container_metadata(&container_name)
                .await?
                .with_context(|| {
                    format!("runner container {container_name} disappeared after start")
                })?;
        }

        self.observe_running(owner, workspace_root, network_enabled, &config, &inspected);
        self.last_active
            .lock()
            .expect("runner activity map poisoned")
            .insert(owner.stable_key(), Instant::now());

        Ok(container_name)
    }

    async fn create_runner(
        &self,
        owner: &RunnerOwner,
        workspace_root: &Path,
        host_workspace_root: &Path,
        container_name: &str,
        network_enabled: bool,
        config: &RunnerManagerConfig,
    ) -> anyhow::Result<()> {
        // Ownership is read from the workspace as visible to runner-manager;
        // the host path is only meaningful to the docker daemon for the bind
        // mount below.
        let user_spec = workspace_owner_user_spec(workspace_root).with_context(|| {
            format!(
                "failed to resolve workspace owner for {}",
                workspace_root.display()
            )
        })?;
        let mut args = vec![
            "run".to_string(),
            "-d".to_string(),
            "--name".to_string(),
            container_name.to_string(),
            "--user".to_string(),
            user_spec,
            "--label".to_string(),
            format!("desk-foreman.owner={}", owner.stable_key()),
            "--label".to_string(),
            "desk-foreman.managed=true".to_string(),
            "--label".to_string(),
            format!("desk-foreman.manager={}", self.manager_id),
            "-e".to_string(),
            format!("WORKSPACE_ROOT={}", config.workdir),
            "-v".to_string(),
            format!("{}:{}", host_workspace_root.display(), config.workdir),
            "-w".to_string(),
            config.workdir.clone(),
            "--read-only".to_string(),
            "--tmpfs".to_string(),
            "/tmp:rw,nosuid,size=256m".to_string(),
            "--cap-drop".to_string(),
            "ALL".to_string(),
            "--security-opt".to_string(),
            "no-new-privileges:true".to_string(),
            "--pids-limit".to_string(),
            config.pids_limit.to_string(),
            "--memory".to_string(),
            config.memory_limit.clone(),
            "--cpus".to_string(),
            config.cpu_limit.clone(),
        ];
        if let Some(runtime_class) = &config.runtime_class {
            args.push("--runtime".to_string());
            args.push(runtime_class.clone());
        }
        if !network_enabled {
            args.push("--network".to_string());
            args.push("none".to_string());
        }
        args.push(config.image.clone());
        args.push("sleep".to_string());
        args.push("infinity".to_string());

        let output = self.docker_output_owned(args, None, None).await?;
        ensure_docker_command_succeeded("create runner container", &output)
    }

    pub(super) async fn docker_status<const N: usize>(
        &self,
        args: [&str; N],
    ) -> anyhow::Result<()> {
        self.docker_output(args).await.map(|_| ())
    }

    pub(super) async fn docker_output<const N: usize>(
        &self,
        args: [&str; N],
    ) -> anyhow::Result<String> {
        self.docker_output_vec(args.iter().map(|value| (*value).to_string()).collect())
            .await
    }

    pub(super) async fn docker_output_owned(
        &self,
        args: Vec<String>,
        timeout_ms: Option<u64>,
        max_output_bytes: Option<usize>,
    ) -> anyhow::Result<CommandOutput> {
        let config = self.config.read().await.clone();
        let mut command = Command::new(&config.docker_cli);
        command.args(args);
        if let Some(host) = &config.docker_host {
            command.env("DOCKER_HOST", host);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        super::bounded_command_output(command, timeout_ms, max_output_bytes.unwrap_or(256 * 1024))
            .await
    }

    async fn docker_output_vec(&self, args: Vec<String>) -> anyhow::Result<String> {
        let config = self.config.read().await.clone();
        let mut command = Command::new(&config.docker_cli);
        command.args(&args);
        if let Some(host) = &config.docker_host {
            command.env("DOCKER_HOST", host);
        }
        let output = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .with_context(|| format!("failed to execute {}", config.docker_cli))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            anyhow::bail!(
                "docker {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }

    pub(super) fn container_workdir(
        config: &RunnerManagerConfig,
        workspace_root: &Path,
        working_dir: &Path,
    ) -> anyhow::Result<String> {
        let relative = working_dir
            .strip_prefix(workspace_root)
            .with_context(|| "working directory escaped workspace root")?;
        let mut target = PathBuf::from(&config.workdir);
        if !relative.as_os_str().is_empty() {
            target.push(relative);
        }
        Ok(target.to_string_lossy().to_string())
    }

    fn host_workspace_root(
        config: &RunnerManagerConfig,
        workspace_root: &Path,
    ) -> anyhow::Result<PathBuf> {
        let relative = workspace_root
            .strip_prefix(&config.workspace_root)
            .with_context(|| "workspace root is outside runner-manager WORKSPACE_ROOT")?;
        Ok(config.host_workspace_root.join(relative))
    }
}

pub(super) fn docker_host_env(config: &RunnerManagerConfig) -> Vec<(String, String)> {
    config
        .docker_host
        .as_ref()
        .map(|value| vec![("DOCKER_HOST".to_string(), value.clone())])
        .unwrap_or_default()
}

/// Resolves the `--user` spec for runner containers from the workspace
/// directory ownership as seen by runner-manager, so the container process can
/// write to the bind-mounted workspace without running as root.
#[cfg(unix)]
fn workspace_owner_user_spec(host_workspace_root: &Path) -> anyhow::Result<String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(host_workspace_root)
        .with_context(|| format!("failed to stat {}", host_workspace_root.display()))?;
    Ok(format!("{}:{}", metadata.uid(), metadata.gid()))
}

#[cfg(not(unix))]
fn workspace_owner_user_spec(_host_workspace_root: &Path) -> anyhow::Result<String> {
    anyhow::bail!("docker runner backend requires a unix host")
}
