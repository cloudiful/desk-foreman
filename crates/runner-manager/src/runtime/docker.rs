use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Instant,
};

use anyhow::Context;
use desk_foreman::runner::RunnerFuture;
use runner_protocol::{CommandOutput, RunnerCommandRequest, RunnerOwner, RunnerShellRequest};
use serde_json::Value;
use tokio::process::Command;

use crate::config::{RunnerManagerConfig, SharedRunnerManagerConfig};

use super::{
    ProcessSpawnTarget, RunnerBackend,
    docker_command::{ensure_docker_command_succeeded, is_missing_container_error},
    shell_spawn::append_shell_args,
};

pub struct DockerRunnerBackend {
    config: SharedRunnerManagerConfig,
    /// Serializes container create/start reconciliation per owner so concurrent
    /// first requests cannot race on `docker run --name <same-name>`.
    runner_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    last_active: Mutex<HashMap<String, Instant>>,
}

impl DockerRunnerBackend {
    pub fn new(config: SharedRunnerManagerConfig) -> Arc<Self> {
        Arc::new(Self {
            config,
            runner_locks: Mutex::new(HashMap::new()),
            last_active: Mutex::new(HashMap::new()),
        })
    }

    fn runner_lock(&self, owner: &RunnerOwner) -> Arc<tokio::sync::Mutex<()>> {
        let key = owner.stable_key().to_string();
        let mut locks = self.runner_locks.lock().expect("runner lock map poisoned");
        locks
            .entry(key)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    async fn ensure_runner(
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
        self.last_active
            .lock()
            .expect("runner activity map poisoned")
            .insert(owner.stable_key(), Instant::now());
        let host_workspace_root = Self::host_workspace_root(&config, workspace_root)?;
        let mut inspected = self.inspect_container(&container_name).await?;
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
            inspected = self.inspect_container(&container_name).await?;
        }

        let inspected = inspected.with_context(|| {
            format!("runner container {container_name} could not be inspected after creation")
        })?;

        if inspected.status != "running" {
            self.docker_status(["start", container_name.as_str()])
                .await?;
        }

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

    async fn inspect_container(
        &self,
        container_name: &str,
    ) -> anyhow::Result<Option<ContainerState>> {
        let output = self
            .docker_output(["inspect", container_name, "--format", "{{json .State}}"])
            .await;
        match output {
            Ok(stdout) => {
                let value: Value = serde_json::from_str(stdout.trim()).with_context(|| {
                    format!("failed to parse docker inspect state for {container_name}")
                })?;
                let status = value
                    .get("Status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string();
                Ok(Some(ContainerState { status }))
            }
            Err(error) if is_missing_container_error(&error.to_string()) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn docker_status<const N: usize>(&self, args: [&str; N]) -> anyhow::Result<()> {
        self.docker_output(args).await.map(|_| ())
    }

    async fn docker_output<const N: usize>(&self, args: [&str; N]) -> anyhow::Result<String> {
        self.docker_output_vec(args.iter().map(|value| (*value).to_string()).collect())
            .await
    }

    async fn docker_output_owned(
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

    fn container_workdir(
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

impl RunnerBackend for DockerRunnerBackend {
    fn reconcile<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn prepare_shell_spawn<'a>(
        &'a self,
        request: RunnerShellRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ProcessSpawnTarget>> {
        Box::pin(async move {
            let container_name = self
                .ensure_runner(
                    &request.owner,
                    &request.workspace_root,
                    request.network_enabled,
                )
                .await?;
            let config = self.config.read().await.clone();
            let workdir =
                Self::container_workdir(&config, &request.workspace_root, &request.working_dir)?;
            let mut args = vec!["exec".to_string(), "-i".to_string()];
            if request.tty {
                args.push("-t".to_string());
            }
            args.push("-w".to_string());
            args.push(workdir);
            args.push("-e".to_string());
            args.push(format!("WORKSPACE_ROOT={}", config.workdir));
            args.push(container_name);
            args.push(request.shell);
            args.extend(append_shell_args(request.login, &request.command));
            Ok(ProcessSpawnTarget {
                program: config.docker_cli.clone(),
                args,
                env: docker_host_env(&config),
                cwd: None,
            })
        })
    }

    fn run_command<'a>(
        &'a self,
        request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
        Box::pin(async move {
            let container_name = self
                .ensure_runner(
                    &request.owner,
                    &request.workspace_root,
                    request.network_enabled,
                )
                .await?;
            let config = self.config.read().await.clone();
            let workdir =
                Self::container_workdir(&config, &request.workspace_root, &request.working_dir)?;
            let mut args = vec![
                "exec".to_string(),
                "-w".to_string(),
                workdir,
                "-e".to_string(),
                format!("WORKSPACE_ROOT={}", config.workdir),
                container_name,
                request.program,
            ];
            args.extend(request.args);
            self.docker_output_owned(args, request.timeout_ms, request.max_output_bytes)
                .await
        })
    }

    fn reclaim_idle_runners<'a>(
        &'a self,
        active_owners: Vec<RunnerOwner>,
    ) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let config = self.config.read().await.clone();
            let active = active_owners
                .into_iter()
                .map(|owner| owner.stable_key())
                .collect::<std::collections::HashSet<_>>();
            let cutoff = Instant::now()
                .checked_sub(config.idle_ttl)
                .unwrap_or_else(Instant::now);
            let stale = {
                let mut activity = self
                    .last_active
                    .lock()
                    .expect("runner activity map poisoned");
                let stale = activity
                    .iter()
                    .filter(|(owner, last)| !active.contains(*owner) && **last <= cutoff)
                    .map(|(owner, _)| owner.clone())
                    .collect::<Vec<_>>();
                for owner in &stale {
                    activity.remove(owner);
                }
                stale
            };
            for owner in stale {
                let container = format!("desk-foreman-runner-{}", owner.replace([':', '/'], "-"));
                let _ = self.docker_status(["stop", container.as_str()]).await;
            }
            Ok(())
        })
    }
}

#[derive(Clone, Debug)]
struct ContainerState {
    status: String,
}

fn docker_host_env(config: &RunnerManagerConfig) -> Vec<(String, String)> {
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
