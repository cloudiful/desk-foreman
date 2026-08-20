use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use super::{docker::DockerRunnerBackend, docker_command::is_missing_container_error};
use crate::config::RunnerManagerConfig;
use anyhow::Context;
use runner_protocol::{RunnerLifecycleEvent, RunnerLifecycleStatus, RunnerOwner};
use serde_json::Value;

impl DockerRunnerBackend {
    pub(crate) async fn inspect_container_metadata(
        &self,
        container_name: &str,
    ) -> anyhow::Result<Option<ContainerMetadata>> {
        let output = self
            .docker_output(["inspect", container_name, "--format", "{{json .}}"])
            .await;
        let stdout = match output {
            Ok(stdout) => stdout,
            Err(error) if is_missing_container_error(&error.to_string()) => return Ok(None),
            Err(error) => return Err(error),
        };
        let value: Value = serde_json::from_str(stdout.trim()).with_context(|| {
            format!("failed to parse docker inspect metadata for {container_name}")
        })?;
        let status = value
            .get("State")
            .and_then(|state| state.get("Status"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let container_id = value.get("Id").and_then(Value::as_str).map(str::to_string);
        let manager_id = value
            .get("Config")
            .and_then(|config| config.get("Labels"))
            .and_then(|labels| labels.get("desk-foreman.manager"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let owner_key = value
            .get("Config")
            .and_then(|config| config.get("Labels"))
            .and_then(|labels| labels.get("desk-foreman.owner"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let workspace_destination = self.config.read().await.workdir.clone();
        let workspace_source = value
            .get("Mounts")
            .and_then(Value::as_array)
            .and_then(|mounts| {
                mounts.iter().find_map(|mount| {
                    (mount.get("Destination").and_then(Value::as_str)
                        == Some(workspace_destination.as_str()))
                    .then(|| mount.get("Source").and_then(Value::as_str))
                    .flatten()
                    .map(PathBuf::from)
                })
            });
        Ok(Some(ContainerMetadata {
            status,
            container_id,
            manager_id,
            owner_key,
            workspace_source,
        }))
    }

    pub(crate) async fn reconcile_containers(&self) -> anyhow::Result<()> {
        let config = self.config.read().await.clone();
        let stdout = match self
            .docker_output([
                "ps",
                "-a",
                "--filter",
                "label=desk-foreman.managed=true",
                "--format",
                "{{.Names}}",
            ])
            .await
        {
            Ok(stdout) => stdout,
            Err(error) => {
                tracing::warn!(error = %error, "failed to list managed containers during reconcile");
                return Ok(());
            }
        };
        for line in stdout.lines() {
            let container_name = line.trim();
            if container_name.is_empty() {
                continue;
            }
            let metadata = match self.inspect_container_metadata(container_name).await {
                Ok(Some(metadata)) => metadata,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(
                        container = %container_name,
                        error = %error,
                        "failed to inspect managed container during reconcile"
                    );
                    continue;
                }
            };
            let owner_matches = metadata
                .owner_key
                .as_deref()
                .and_then(owner_from_stable_key)
                .is_some_and(|owner| owner.container_name() == container_name);
            let manager_owned =
                owner_matches && metadata.manager_id.as_deref() == Some(self.manager_id.as_str());
            let legacy_owned = owner_matches
                && metadata.manager_id.is_none()
                && legacy_workspace_mount_matches(&config, metadata.workspace_source.as_deref());
            if !manager_owned && !legacy_owned {
                continue;
            }
            if matches!(metadata.status.as_str(), "running" | "restarting")
                && metadata
                    .owner_key
                    .as_deref()
                    .is_some_and(|owner| self.has_active_operation(owner))
            {
                continue;
            }
            if let Err(error) = self.stop_and_remove(container_name).await {
                self.report_cleanup_failed_metadata(&metadata, container_name, &error);
                tracing::warn!(
                    container = %container_name,
                    error = %error,
                    "failed to remove managed container during reconcile"
                );
            } else {
                self.report_removed_metadata(&metadata, container_name);
            }
        }
        Ok(())
    }

    pub(crate) async fn reclaim_idle_runners_inner(&self) {
        let config = self.config.read().await.clone();
        let cutoff = Instant::now()
            .checked_sub(config.idle_ttl)
            .unwrap_or_else(Instant::now);
        let candidates = {
            let activity = self
                .last_active
                .lock()
                .expect("runner activity map poisoned");
            let ops = self.active_ops.lock().expect("active ops map poisoned");
            activity
                .iter()
                .filter(|(owner, last)| {
                    ops.get(*owner).copied().unwrap_or(0) == 0 && **last <= cutoff
                })
                .filter_map(|(owner, _)| owner_from_stable_key(owner))
                .collect::<Vec<_>>()
        };
        for owner in candidates {
            let owner_key = owner.stable_key();
            let lock = self.runner_lock(&owner);
            let _guard = lock.lock().await;
            let should_reclaim = {
                let activity = self
                    .last_active
                    .lock()
                    .expect("runner activity map poisoned");
                let ops = self.active_ops.lock().expect("active ops map poisoned");
                ops.get(&owner_key).copied().unwrap_or(0) == 0
                    && activity.get(&owner_key).is_some_and(|last| *last <= cutoff)
            };
            if !should_reclaim {
                continue;
            }
            let container = owner.container_name();
            match self.stop_and_remove(&container).await {
                Ok(()) => {
                    self.report_removed(&container);
                    self.last_active
                        .lock()
                        .expect("runner activity map poisoned")
                        .remove(&owner_key);
                }
                Err(error) => {
                    self.report_cleanup_failed(&container, &error);
                    tracing::warn!(
                        owner = %owner_key,
                        container = %container,
                        error = %error,
                        "failed to reclaim idle runner; retaining cleanup state for retry"
                    );
                }
            }
        }
    }

    pub(crate) fn has_active_operation(&self, owner: &str) -> bool {
        let ops = self.active_ops.lock().expect("active ops map poisoned");
        ops.get(owner).copied().unwrap_or(0) > 0
    }

    pub(crate) fn bump_active(&self, owner_key: &str) {
        let mut ops = self.active_ops.lock().expect("active ops map poisoned");
        *ops.entry(owner_key.to_string()).or_insert(0) += 1;
    }

    pub(crate) fn touch_runner_activity(&self, owner: &RunnerOwner) {
        self.last_active
            .lock()
            .expect("runner activity map poisoned")
            .insert(owner.stable_key(), Instant::now());
        if let Some(event) = self
            .observed
            .lock()
            .expect("runner observation map poisoned")
            .get(&owner.container_name())
            .cloned()
        {
            self.reporter.report(event);
        }
    }

    pub(crate) fn decrement_active(&self, owner_key: &str) {
        let mut ops = self.active_ops.lock().expect("active ops map poisoned");
        if let Some(count) = ops.get_mut(owner_key) {
            if *count > 0 {
                *count -= 1;
            }
            if *count == 0 {
                ops.remove(owner_key);
            }
        }
    }

    pub(crate) async fn stop_and_remove(&self, container_name: &str) -> anyhow::Result<()> {
        self.stop_and_remove_inner(container_name).await
    }

    async fn stop_and_remove_inner(&self, container_name: &str) -> anyhow::Result<()> {
        if let Err(error) = self.docker_status(["stop", container_name]).await {
            if is_missing_container_error(&error.to_string()) {
                return Ok(());
            }
            tracing::debug!(%container_name, error = %error, "docker stop failed during cleanup");
        }
        match self.docker_status(["rm", container_name]).await {
            Ok(()) => Ok(()),
            Err(error) => {
                let message = error.to_string();
                if is_missing_container_error(&message) {
                    Ok(())
                } else {
                    self.docker_status(["rm", "-f", container_name])
                        .await
                        .map_err(|force_error| {
                            anyhow::anyhow!(
                                "failed to remove container {container_name}: {error}; force removal failed: {force_error}"
                            )
                        })
                }
            }
        }
    }

    pub(crate) fn observe_running(
        &self,
        owner: &RunnerOwner,
        workspace_root: &Path,
        network_enabled: bool,
        config: &RunnerManagerConfig,
        metadata: &ContainerMetadata,
    ) {
        let event = RunnerLifecycleEvent {
            owner: owner.clone(),
            container_name: owner.container_name(),
            container_id: metadata.container_id.clone(),
            status: RunnerLifecycleStatus::Running,
            workspace_root: Some(workspace_root.to_string_lossy().to_string()),
            runtime: Some("docker".to_string()),
            runtime_class: config.runtime_class.clone(),
            image_name: Some(config.image.clone()),
            network_enabled: Some(network_enabled),
            last_error: None,
        };
        self.observed
            .lock()
            .expect("runner observation map poisoned")
            .insert(event.container_name.clone(), event.clone());
        self.reporter.report(event);
    }

    pub(crate) fn report_removed(&self, container_name: &str) {
        let Some(mut event) = self
            .observed
            .lock()
            .expect("runner observation map poisoned")
            .remove(container_name)
        else {
            return;
        };
        event.status = RunnerLifecycleStatus::Removed;
        event.last_error = None;
        self.reporter.report(event);
    }

    pub(crate) fn report_cleanup_failed(&self, container_name: &str, error: &anyhow::Error) {
        let Some(mut event) = self
            .observed
            .lock()
            .expect("runner observation map poisoned")
            .get(container_name)
            .cloned()
        else {
            return;
        };
        event.status = RunnerLifecycleStatus::CleanupFailed;
        event.last_error = Some(error.to_string());
        self.reporter.report(event);
    }

    fn report_removed_metadata(&self, metadata: &ContainerMetadata, container_name: &str) {
        let Some(owner_key) = metadata
            .owner_key
            .as_deref()
            .and_then(owner_from_stable_key)
        else {
            return;
        };
        self.reporter.report(RunnerLifecycleEvent {
            owner: owner_key,
            container_name: container_name.to_string(),
            container_id: metadata.container_id.clone(),
            status: RunnerLifecycleStatus::Removed,
            workspace_root: None,
            runtime: None,
            runtime_class: None,
            image_name: None,
            network_enabled: None,
            last_error: None,
        });
    }

    fn report_cleanup_failed_metadata(
        &self,
        metadata: &ContainerMetadata,
        container_name: &str,
        error: &anyhow::Error,
    ) {
        let Some(owner_key) = metadata
            .owner_key
            .as_deref()
            .and_then(owner_from_stable_key)
        else {
            return;
        };
        self.reporter.report(RunnerLifecycleEvent {
            owner: owner_key,
            container_name: container_name.to_string(),
            container_id: metadata.container_id.clone(),
            status: RunnerLifecycleStatus::CleanupFailed,
            workspace_root: None,
            runtime: None,
            runtime_class: None,
            image_name: None,
            network_enabled: None,
            last_error: Some(error.to_string()),
        });
    }
}

fn owner_from_stable_key(key: &str) -> Option<RunnerOwner> {
    if let Some(rest) = key.strip_prefix("user:") {
        Some(RunnerOwner::InternalUser {
            user_id: rest.parse().ok()?,
        })
    } else if let Some(rest) = key.strip_prefix("workspace_binding:") {
        Some(RunnerOwner::WorkspaceBinding {
            workspace_binding_id: rest.parse().ok()?,
        })
    } else {
        None
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ContainerMetadata {
    pub(crate) status: String,
    pub(crate) container_id: Option<String>,
    pub(crate) manager_id: Option<String>,
    pub(crate) owner_key: Option<String>,
    pub(crate) workspace_source: Option<PathBuf>,
}

pub(crate) fn legacy_workspace_mount_matches(
    config: &RunnerManagerConfig,
    workspace_source: Option<&Path>,
) -> bool {
    workspace_source.is_some_and(|source| source.starts_with(&config.host_workspace_root))
}
