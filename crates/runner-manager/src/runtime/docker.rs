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

use super::execution_environment::service::ServiceCancelRegistry;
use super::{RunnerLifecycleReporter, docker_command::ensure_docker_command_succeeded};
use tokio::time::{Duration, timeout};

pub struct DockerRunnerBackend {
    pub(crate) config: SharedRunnerManagerConfig,
    pub(crate) manager_id: String,
    pub(crate) reporter: Arc<RunnerLifecycleReporter>,
    pub(crate) observed: Mutex<HashMap<String, runner_protocol::RunnerLifecycleEvent>>,
    /// Last container id whose first Running observation the control plane
    /// acknowledged, keyed by owner. A changed id re-acknowledges.
    pub(crate) reported: Mutex<HashMap<String, Option<String>>>,
    /// Serializes container create/start reconciliation per owner so concurrent
    /// first requests cannot race on `docker run --name <same-name>`.
    pub(crate) runner_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    pub(crate) last_active: Mutex<HashMap<String, Instant>>,
    pub(crate) active_ops: Mutex<HashMap<String, usize>>,
    /// Wakes live relayed service streams when `stop` persists a stopped
    /// generation. Network bookkeeping only: the durable barrier stays the
    /// authority on what may serve.
    pub(crate) service_cancel: ServiceCancelRegistry,
    /// Filesystem materializer for native-task checkouts. Disabled when no
    /// trusted root is configured; it never falls back to a workspace root.
    pub(crate) native_tasks: crate::runtime::NativeTaskStore,
}

impl DockerRunnerBackend {
    pub fn new(
        config: SharedRunnerManagerConfig,
        manager_id: String,
        reporter: Arc<RunnerLifecycleReporter>,
        native_tasks: crate::runtime::NativeTaskStore,
    ) -> Arc<Self> {
        Arc::new(Self {
            config,
            manager_id,
            reporter,
            observed: Mutex::new(HashMap::new()),
            reported: Mutex::new(HashMap::new()),
            runner_locks: Mutex::new(HashMap::new()),
            last_active: Mutex::new(HashMap::new()),
            active_ops: Mutex::new(HashMap::new()),
            service_cancel: ServiceCancelRegistry::new(),
            native_tasks,
        })
    }

    /// Wakes live relayed streams of one environment after a committed stop.
    pub(crate) fn service_cancel(&self) -> &ServiceCancelRegistry {
        &self.service_cancel
    }

    pub(crate) fn runner_lock(&self, owner: &RunnerOwner) -> Arc<tokio::sync::Mutex<()>> {
        let key = owner.lock_key();
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
        // Legacy workspace/user path only. An execution-environment owner must
        // use the environment ensure primitive; reject it here, before any
        // Docker command or mount construction runs.
        if runner_protocol::execution_environment::is_execution_environment(owner) {
            anyhow::bail!(
                "execution environment owners must use ensure_execution_environment, not ensure_runner"
            );
        }
        let config = self.config.read().await.clone();
        let lock = self.runner_lock(owner);
        let _guard = lock.lock().await;
        let network_enabled = requested_network_enabled && config.network_enabled;
        let container_name = owner.container_name();
        let host_workspace_root = Self::workspace_mount(&config, workspace_root)?;
        let mut inspected = self.inspect_container_metadata(&container_name).await?;
        if let Some(metadata) = &inspected {
            if matches!(owner, RunnerOwner::ApplicationPrincipal { .. }) {
                super::principal_admission::verified_id(self, owner, metadata)?;
            }
            let same_owner = metadata.owner_key.as_deref() == Some(owner.stable_key().as_str());
            let exact_mount =
                metadata.workspace_source.as_deref() == Some(host_workspace_root.as_path());
            if metadata.manager_id.as_deref() == Some(self.manager_id.as_str()) && same_owner {
                // The current manager's existing container is reused only when
                // it already mounts the exact expected child; never start a
                // wide or foreign layout.
                if !exact_mount {
                    anyhow::bail!("{}", incompatible_workspace_mount(&container_name));
                }
            } else if metadata.manager_id.is_none() && same_owner {
                // A legacy same-owner container may still recreate, but only
                // when its inspected mount is exactly the expected child.
                // Any other mount is incompatible and left untouched.
                if !exact_mount
                    || !super::docker_lifecycle::legacy_workspace_mount_matches(
                        &config,
                        metadata.workspace_source.as_deref(),
                    )
                {
                    anyhow::bail!("{}", incompatible_workspace_mount(&container_name));
                }
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

        if matches!(owner, RunnerOwner::ApplicationPrincipal { .. }) {
            super::principal_admission::verified_id(self, owner, &inspected)?;
        }
        if inspected.status != "running" {
            let target = if matches!(owner, RunnerOwner::ApplicationPrincipal { .. }) {
                super::principal_admission::verified_id(self, owner, &inspected)?
            } else {
                container_name.as_str()
            };
            self.docker_status(["start", target]).await?;
            inspected = self
                .inspect_container_metadata(&container_name)
                .await?
                .with_context(|| {
                    format!("runner container {container_name} disappeared after start")
                })?;
        }

        // Every ordinary owner variant must prove the exact expected mount on
        // the freshly inspected container, including a newly created one, so a
        // reuse/start never returns behind a wide or foreign layout.
        if inspected.workspace_source.as_deref() != Some(host_workspace_root.as_path()) {
            anyhow::bail!("{}", incompatible_workspace_mount(&container_name));
        }

        let target = if matches!(owner, RunnerOwner::ApplicationPrincipal { .. }) {
            let id = super::principal_admission::verified_id(self, owner, &inspected)?;
            super::principal_admission::admit(self, owner, id).await?;
            id.to_owned()
        } else {
            container_name.clone()
        };
        self.observe_running(owner, workspace_root, network_enabled, &config, &inspected)
            .await?;
        self.last_active
            .lock()
            .expect("runner activity map poisoned")
            .insert(owner.stable_key(), Instant::now());

        Ok(target)
    }

    /// Bounded container termination for lease-takeover quiescence.
    ///
    /// Stops and removes the task-owned runner container (`docker stop` +
    /// `docker rm`, with `rm -f` fallback inside `stop_and_remove`). This
    /// is the real Docker execution stop: killing the local `docker exec`
    /// CLI alone leaves the exec'd shell running inside the container, so
    /// takeover must remove the container to guarantee no old-owner writes
    /// survive. Workspace files are preserved because the workspace is a
    /// host bind mount; only container processes and writable layers go
    /// away. The next `ensure_runner` recreates the container on demand,
    /// so retry after a successful takeover is safe. Missing containers
    /// map to `Ok` (already quiescent); any other failure is `Err` so the
    /// caller fails closed instead of reporting false quiescence. The wait
    /// is bounded so a wedged daemon cannot pin the takeover row lock.
    pub(crate) async fn terminate_container_for_takeover(
        &self,
        container_name: &str,
    ) -> anyhow::Result<()> {
        self.terminate_container_for_takeover_with_timeout(container_name, Duration::from_secs(20))
            .await
    }

    pub(crate) async fn terminate_container_for_takeover_with_timeout(
        &self,
        container_name: &str,
        timeout_duration: Duration,
    ) -> anyhow::Result<()> {
        match timeout(timeout_duration, self.stop_and_remove(container_name)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(error),
            Err(_) => anyhow::bail!(
                "timed out after {timeout_duration:?} terminating container {container_name} for takeover"
            ),
        }
    }

    /// Environment-level lock: all generations of one environment share
    /// one mutex so concurrent ensures cannot attach the same durable
    /// state to two running generations. Legacy owners keep their
    /// stable-key lock via `lock_key()`.
    pub(crate) fn environment_runner_lock(
        &self,
        environment_id: &str,
    ) -> Arc<tokio::sync::Mutex<()>> {
        let key = runner_protocol::execution_environment::environment_lock_key(environment_id);
        let mut locks = self.runner_locks.lock().expect("runner lock map poisoned");
        locks
            .entry(key)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    pub(crate) async fn docker_status<const N: usize>(
        &self,
        args: [&str; N],
    ) -> anyhow::Result<()> {
        self.docker_output(args).await.map(|_| ())
    }

    /// Bounded Docker invocation for execution-environment operations.
    ///
    /// Reuses `bounded_command_output`, so a deadline kills and reaps the
    /// spawned Docker CLI before returning instead of dropping an orphaned
    /// process. Additive: legacy `docker_output`/`docker_status` semantics
    /// are untouched.
    pub(crate) async fn docker_output_bounded(
        &self,
        args: Vec<String>,
        timeout_ms: u64,
    ) -> anyhow::Result<CommandOutput> {
        self.docker_output_owned(args, Some(timeout_ms), None).await
    }

    /// Bounded Docker status: a non-zero exit or a timeout is an error, so
    /// a deadline is never mistaken for a successful command.
    pub(crate) async fn docker_status_bounded(
        &self,
        args: Vec<String>,
        timeout_ms: u64,
    ) -> anyhow::Result<()> {
        let output = self
            .docker_output_owned(args, Some(timeout_ms), None)
            .await?;
        if output.timed_out {
            anyhow::bail!(
                "bounded docker command timed out within the execution environment operation deadline"
            );
        }
        ensure_docker_command_succeeded("run bounded docker command", &output)
    }

    pub(crate) async fn docker_output<const N: usize>(
        &self,
        args: [&str; N],
    ) -> anyhow::Result<String> {
        self.docker_output_vec(args.iter().map(|value| (*value).to_string()).collect())
            .await
    }

    pub(crate) async fn docker_output_owned(
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
            .map_err(|error| {
                // Operators need to tell "the daemon is busy" from "that path
                // is gone" without reading the full error chain. Only the
                // bounded standard classification is added: never the argv,
                // the environment or any child output.
                let kind = error.kind();
                let os_code = match error.raw_os_error() {
                    Some(code) => code.to_string(),
                    None => "none".to_string(),
                };
                anyhow::Error::new(error).context(format!(
                    "failed to execute {}: {kind} (os error {os_code})",
                    config.docker_cli
                ))
            })?;
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

    pub(super) fn workspace_mount(
        config: &RunnerManagerConfig,
        workspace_root: &Path,
    ) -> anyhow::Result<PathBuf> {
        if !workspace_root.is_absolute() {
            anyhow::bail!("workspace root must be an absolute path");
        }
        let trusted_base = config
            .workspace_root
            .canonicalize()
            .context("trusted workspace root is unavailable")?;
        if !trusted_base.is_dir() {
            anyhow::bail!("trusted workspace root is not a directory");
        }
        let requested = workspace_root
            .canonicalize()
            .context("workspace root is unavailable")?;
        if !requested.is_dir() {
            anyhow::bail!("workspace root is not a directory");
        }
        let canonical_relative = requested
            .strip_prefix(&trusted_base)
            .ok()
            .filter(|relative| !relative.as_os_str().is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "workspace root must be a strict descendant of the trusted workspace root"
                )
            })?;

        // Walk the component names from the original request under the
        // canonical trusted base. Canonicalizing first would accept an alias
        // symlink that resolves elsewhere inside the same trusted tree.
        let original_relative = workspace_root
            .strip_prefix(&config.workspace_root)
            .map_err(|_| {
                anyhow::anyhow!("workspace root must be under the trusted workspace root")
            })?;
        let mut current = trusted_base.clone();
        for component in original_relative.components() {
            let std::path::Component::Normal(name) = component else {
                anyhow::bail!("workspace root contains a non-normal path component");
            };
            current.push(name);
            let metadata = std::fs::symlink_metadata(&current)
                .with_context(|| "workspace root component is unavailable".to_string())?;
            if metadata.file_type().is_symlink() {
                anyhow::bail!("workspace root may not traverse a symbolic link");
            }
        }

        let source = config.host_workspace_root.join(canonical_relative);
        validate_mount_value(&source, "workspace mount source")?;
        validate_container_workdir(&config.workdir)?;
        Ok(source)
    }
}

fn incompatible_workspace_mount(container_name: &str) -> String {
    format!(
        "runner container {container_name} has an incompatible workspace mount; operator review and manual cleanup are required"
    )
}

fn validate_mount_value(path: &Path, what: &str) -> anyhow::Result<()> {
    let Some(text) = path.to_str() else {
        anyhow::bail!("{what} must be valid UTF-8");
    };
    if text.is_empty()
        || text
            .bytes()
            .any(|byte| byte == b',' || byte == 0 || byte.is_ascii_control())
    {
        anyhow::bail!("{what} contains a forbidden character");
    }
    Ok(())
}

fn validate_container_workdir(workdir: &str) -> anyhow::Result<()> {
    validate_mount_value(Path::new(workdir), "container workdir")?;
    let path = Path::new(workdir);
    if !path.is_absolute() {
        anyhow::bail!("container workdir must be absolute");
    }
    let mut normalized = PathBuf::from("/");
    for component in path.components() {
        match component {
            std::path::Component::RootDir => {}
            std::path::Component::Normal(name) => normalized.push(name),
            _ => anyhow::bail!("container workdir must be normalized"),
        }
    }
    if normalized == Path::new("/") || normalized.as_os_str() != path.as_os_str() {
        anyhow::bail!("container workdir must be a normalized absolute non-root path");
    }
    Ok(())
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
pub(super) fn workspace_owner_user_spec(host_workspace_root: &Path) -> anyhow::Result<String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(host_workspace_root)
        .with_context(|| format!("failed to stat {}", host_workspace_root.display()))?;
    Ok(format!("{}:{}", metadata.uid(), metadata.gid()))
}

#[cfg(not(unix))]
fn workspace_owner_user_spec(_host_workspace_root: &Path) -> anyhow::Result<String> {
    anyhow::bail!("docker runner backend requires a unix host")
}
