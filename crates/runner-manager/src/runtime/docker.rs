use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use anyhow::Context;
use chrono::{Duration as ChronoDuration, Utc};
use desk_foreman::{
    db::{self, workspace_runners::SaveWorkspaceRunner},
    runner::RunnerFuture,
};
use runner_protocol::{CommandOutput, RunnerCommandRequest, RunnerOwner, RunnerShellRequest};
use serde_json::Value;
use tokio::process::Command;

use crate::config::RunnerManagerConfig;

use super::{ProcessSpawnTarget, RunnerBackend, shell_spawn::append_shell_args};

pub struct DockerRunnerBackend {
    db: sqlx::PgPool,
    config: RunnerManagerConfig,
}

impl DockerRunnerBackend {
    pub fn new(db: sqlx::PgPool, config: RunnerManagerConfig) -> Arc<Self> {
        Arc::new(Self { db, config })
    }

    async fn ensure_runner(
        &self,
        owner: &RunnerOwner,
        workspace_root: &Path,
        requested_network_enabled: bool,
    ) -> anyhow::Result<String> {
        let network_enabled = requested_network_enabled && self.config.network_enabled;
        let container_name = owner.container_name();
        let host_workspace_root = self.host_workspace_root(workspace_root)?;
        if let Some(record) =
            db::workspace_runners::find_workspace_runner_by_owner(&self.db, owner).await?
            && record.network_enabled != network_enabled
        {
            let _ = self
                .docker_status(["rm", "-f", container_name.as_str()])
                .await;
        }
        let mut inspected = self.inspect_container(&container_name).await?;
        if inspected.is_none() {
            self.create_runner(
                owner,
                workspace_root,
                &host_workspace_root,
                &container_name,
                network_enabled,
            )
            .await?;
            inspected = self.inspect_container(&container_name).await?;
        }

        let mut inspected = inspected.with_context(|| {
            format!("runner container {container_name} could not be inspected after creation")
        })?;

        if inspected.status != "running" {
            self.docker_status(["start", container_name.as_str()])
                .await?;
            inspected = self
                .inspect_container(&container_name)
                .await?
                .with_context(|| {
                    format!("runner container {container_name} missing after start")
                })?;
        }

        db::workspace_runners::save_workspace_runner(
            &self.db,
            SaveWorkspaceRunner {
                owner,
                container_name: &container_name,
                container_id: Some(&inspected.id),
                runtime: "docker",
                runtime_class: self.config.runtime_class.as_deref(),
                image_name: &self.config.image,
                status: &inspected.status,
                network_enabled,
                workspace_root: &workspace_root.to_string_lossy(),
                last_error: None,
            },
        )
        .await?;

        Ok(container_name)
    }

    async fn create_runner(
        &self,
        owner: &RunnerOwner,
        workspace_root: &Path,
        host_workspace_root: &Path,
        container_name: &str,
        network_enabled: bool,
    ) -> anyhow::Result<()> {
        let mut args = vec![
            "run".to_string(),
            "-d".to_string(),
            "--name".to_string(),
            container_name.to_string(),
            "--label".to_string(),
            format!("desk-foreman.owner={}", owner.stable_key()),
            "--label".to_string(),
            "desk-foreman.managed=true".to_string(),
            "-e".to_string(),
            format!("WORKSPACE_ROOT={}", self.config.workdir),
            "-v".to_string(),
            format!("{}:{}", host_workspace_root.display(), self.config.workdir),
            "-w".to_string(),
            self.config.workdir.clone(),
            "--read-only".to_string(),
            "--tmpfs".to_string(),
            "/tmp:rw,nosuid,size=256m".to_string(),
            "--cap-drop".to_string(),
            "ALL".to_string(),
            "--security-opt".to_string(),
            "no-new-privileges:true".to_string(),
            "--pids-limit".to_string(),
            self.config.pids_limit.to_string(),
            "--memory".to_string(),
            self.config.memory_limit.clone(),
            "--cpus".to_string(),
            self.config.cpu_limit.clone(),
        ];
        if let Some(runtime_class) = &self.config.runtime_class {
            args.push("--runtime".to_string());
            args.push(runtime_class.clone());
        }
        if !network_enabled {
            args.push("--network".to_string());
            args.push("none".to_string());
        }
        args.push(self.config.image.clone());
        args.push("sleep".to_string());
        args.push("infinity".to_string());

        let output = self.docker_output_owned(args, None, None).await;
        if let Err(error) = output {
            let _ = db::workspace_runners::save_workspace_runner(
                &self.db,
                SaveWorkspaceRunner {
                    owner,
                    container_name,
                    container_id: None,
                    runtime: "docker",
                    runtime_class: self.config.runtime_class.as_deref(),
                    image_name: &self.config.image,
                    status: "error",
                    network_enabled,
                    workspace_root: &workspace_root.to_string_lossy(),
                    last_error: Some(&error.to_string()),
                },
            )
            .await;
            return Err(error);
        }
        Ok(())
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
                let id = self
                    .docker_output(["inspect", container_name, "--format", "{{.Id}}"])
                    .await?
                    .trim()
                    .to_string();
                Ok(Some(ContainerState { id, status }))
            }
            Err(error) if error.to_string().contains("No such object") => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn reconcile_runner_record(
        &self,
        record: &db::workspace_runners::WorkspaceRunnerRecord,
    ) -> anyhow::Result<()> {
        match self.inspect_container(&record.container_name).await? {
            Some(inspected) => {
                let owner = record_owner(record)?;
                db::workspace_runners::save_workspace_runner(
                    &self.db,
                    SaveWorkspaceRunner {
                        owner: &owner,
                        container_name: &record.container_name,
                        container_id: Some(&inspected.id),
                        runtime: &record.runtime,
                        runtime_class: record.runtime_class.as_deref(),
                        image_name: &record.image_name,
                        status: &inspected.status,
                        network_enabled: record.network_enabled,
                        workspace_root: &record.workspace_root,
                        last_error: None,
                    },
                )
                .await?;
            }
            None => {
                let owner = record_owner(record)?;
                db::workspace_runners::save_workspace_runner(
                    &self.db,
                    SaveWorkspaceRunner {
                        owner: &owner,
                        container_name: &record.container_name,
                        container_id: None,
                        runtime: &record.runtime,
                        runtime_class: record.runtime_class.as_deref(),
                        image_name: &record.image_name,
                        status: "missing",
                        network_enabled: record.network_enabled,
                        workspace_root: &record.workspace_root,
                        last_error: Some("runner container missing"),
                    },
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn stop_runner(&self, container_name: &str) -> anyhow::Result<()> {
        self.docker_status(["stop", container_name]).await?;
        Ok(())
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
        let mut command = Command::new(&self.config.docker_cli);
        command.args(args);
        if let Some(host) = &self.config.docker_host {
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
        let mut command = Command::new(&self.config.docker_cli);
        command.args(&args);
        if let Some(host) = &self.config.docker_host {
            command.env("DOCKER_HOST", host);
        }
        let output = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .with_context(|| format!("failed to execute {}", self.config.docker_cli))?;
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
        &self,
        workspace_root: &Path,
        working_dir: &Path,
    ) -> anyhow::Result<String> {
        let relative = working_dir
            .strip_prefix(workspace_root)
            .with_context(|| "working directory escaped workspace root")?;
        let mut target = PathBuf::from(&self.config.workdir);
        if !relative.as_os_str().is_empty() {
            target.push(relative);
        }
        Ok(target.to_string_lossy().to_string())
    }

    fn host_workspace_root(&self, workspace_root: &Path) -> anyhow::Result<PathBuf> {
        let relative = workspace_root
            .strip_prefix(&self.config.workspace_root)
            .with_context(|| "workspace root is outside runner-manager WORKSPACE_ROOT")?;
        Ok(self.config.host_workspace_root.join(relative))
    }
}

impl RunnerBackend for DockerRunnerBackend {
    fn reconcile<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            for record in db::workspace_runners::list_workspace_runners(&self.db).await? {
                self.reconcile_runner_record(&record).await?;
            }
            Ok(())
        })
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
            let workdir = self.container_workdir(&request.workspace_root, &request.working_dir)?;
            let mut args = vec!["exec".to_string(), "-i".to_string()];
            if request.tty {
                args.push("-t".to_string());
            }
            args.push("-w".to_string());
            args.push(workdir);
            args.push("-e".to_string());
            args.push(format!("WORKSPACE_ROOT={}", self.config.workdir));
            args.push(container_name);
            args.push(request.shell);
            args.extend(append_shell_args(request.login, &request.command));
            Ok(ProcessSpawnTarget {
                program: self.config.docker_cli.clone(),
                args,
                env: docker_host_env(&self.config),
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
            let workdir = self.container_workdir(&request.workspace_root, &request.working_dir)?;
            db::workspace_runners::touch_workspace_runner(&self.db, &request.owner).await?;
            let mut args = vec![
                "exec".to_string(),
                "-w".to_string(),
                workdir,
                "-e".to_string(),
                format!("WORKSPACE_ROOT={}", self.config.workdir),
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
            let excluded = active_owners
                .into_iter()
                .map(|owner| owner.stable_key())
                .collect::<HashSet<_>>();
            let idle_before = Utc::now()
                - ChronoDuration::from_std(self.config.idle_ttl)
                    .context("invalid runner idle ttl")?;
            for record in
                db::workspace_runners::list_stale_workspace_runners(&self.db, idle_before).await?
            {
                let owner = record_owner(&record)?;
                if excluded.contains(&owner.stable_key()) {
                    continue;
                }
                if self
                    .inspect_container(&record.container_name)
                    .await?
                    .is_some()
                {
                    self.stop_runner(&record.container_name).await?;
                }
                db::workspace_runners::save_workspace_runner(
                    &self.db,
                    SaveWorkspaceRunner {
                        owner: &owner,
                        container_name: &record.container_name,
                        container_id: record.container_id.as_deref(),
                        runtime: &record.runtime,
                        runtime_class: record.runtime_class.as_deref(),
                        image_name: &record.image_name,
                        status: "stopped",
                        network_enabled: record.network_enabled,
                        workspace_root: &record.workspace_root,
                        last_error: None,
                    },
                )
                .await?;
            }
            Ok(())
        })
    }
}

#[derive(Clone, Debug)]
struct ContainerState {
    id: String,
    status: String,
}

fn docker_host_env(config: &RunnerManagerConfig) -> Vec<(String, String)> {
    config
        .docker_host
        .as_ref()
        .map(|value| vec![("DOCKER_HOST".to_string(), value.clone())])
        .unwrap_or_default()
}

fn record_owner(
    record: &db::workspace_runners::WorkspaceRunnerRecord,
) -> anyhow::Result<RunnerOwner> {
    match record.owner_kind.as_str() {
        "user" => Ok(RunnerOwner::InternalUser {
            user_id: record
                .owner_user_id
                .with_context(|| "workspace runner record missing owner_user_id")?,
        }),
        "workspace_binding" => Ok(RunnerOwner::WorkspaceBinding {
            workspace_binding_id: record
                .owner_workspace_binding_id
                .with_context(|| "workspace runner record missing owner_workspace_binding_id")?,
        }),
        other => anyhow::bail!("unsupported workspace runner owner kind: {other}"),
    }
}
