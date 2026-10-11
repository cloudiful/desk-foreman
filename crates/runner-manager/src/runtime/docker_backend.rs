use desk_foreman::runner::RunnerFuture;
use runner_protocol::{
    CommandOutput, EnsureExecutionEnvironmentAck, EnsureExecutionEnvironmentRequest,
    NativeTaskPreparedResult, PrepareNativeTaskRequest, RunnerCommandRequest, RunnerOwner,
    RunnerShellRequest, SeedBundle, StartNativeTaskRequest, StartNativeTaskResult,
    StopExecutionEnvironmentAck, StopExecutionEnvironmentRequest, StopNativeTaskRequest,
    StopNativeTaskResult, execution_environment as protocol,
};

use super::{
    ProcessSpawnTarget, RunnerBackend, backend::BorrowedRunnerOperationLease,
    docker::DockerRunnerBackend, execution_environment::ExecutionEnvironmentProfile,
    shell_spawn::append_shell_args,
};

impl RunnerBackend for DockerRunnerBackend {
    fn reconcile<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async move { self.reconcile_containers().await })
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
                env: crate::runtime::docker::docker_host_env(&config),
                cwd: None,
            })
        })
    }

    fn prepare_native_task<'a>(
        &'a self,
        request: PrepareNativeTaskRequest,
        seed: SeedBundle,
    ) -> RunnerFuture<'a, anyhow::Result<NativeTaskPreparedResult>> {
        Box::pin(async move { self.prepare_native_task_checkout(request, seed).await })
    }

    fn start_native_task<'a>(
        &'a self,
        request: StartNativeTaskRequest,
    ) -> RunnerFuture<'a, anyhow::Result<StartNativeTaskResult>> {
        Box::pin(async move {
            let profile = ExecutionEnvironmentProfile::from_env_or_default();
            super::start_native_task(self, &profile, request).await
        })
    }

    fn stop_native_task<'a>(
        &'a self,
        request: StopNativeTaskRequest,
    ) -> RunnerFuture<'a, anyhow::Result<StopNativeTaskResult>> {
        Box::pin(async move {
            let profile = ExecutionEnvironmentProfile::from_env_or_default();
            super::stop_native_task(self, &profile, request).await
        })
    }

    fn run_command<'a>(
        &'a self,
        request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
        Box::pin(async move {
            let _operation = BorrowedRunnerOperationLease::new(self, request.owner.clone());
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

    fn begin_shell_operation(&self, owner: &RunnerOwner) {
        self.bump_active(&owner.stable_key());
    }

    fn end_shell_operation(&self, owner: &RunnerOwner) {
        self.decrement_active(&owner.stable_key());
    }

    fn touch_activity(&self, owner: &RunnerOwner) {
        self.touch_runner_activity(owner);
    }

    fn cleanup_runner_owner<'a>(
        &'a self,
        owner: RunnerOwner,
    ) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            if runner_protocol::execution_environment::is_execution_environment(&owner) {
                anyhow::bail!(
                    "execution environment owners must use stop_execution_environment, not cleanup_runner_owner"
                );
            }
            let lock = self.runner_lock(&owner);
            let _guard = lock.lock().await;
            if self
                .active_ops
                .lock()
                .expect("active ops map poisoned")
                .get(&owner.stable_key())
                .copied()
                .unwrap_or(0)
                > 0
            {
                anyhow::bail!("runner owner {} has active operations", owner.stable_key());
            }
            let container_name = owner.container_name();
            // Bounded real container stop (not just local CLI kill): the
            // workspace bind mount preserves files while all exec
            // processes inside die with the container. Fail closed so a
            // wedged daemon blocks takeover instead of reporting false
            // quiescence.
            match self.terminate_container_for_takeover(&container_name).await {
                Ok(()) => self.report_removed(&container_name),
                Err(error) => {
                    self.report_cleanup_failed(&container_name, &error);
                    return Err(error);
                }
            }
            self.last_active
                .lock()
                .expect("runner activity map poisoned")
                .remove(&owner.stable_key());
            Ok(())
        })
    }

    fn ensure_execution_environment<'a>(
        &'a self,
        request: EnsureExecutionEnvironmentRequest,
    ) -> RunnerFuture<'a, anyhow::Result<EnsureExecutionEnvironmentAck>> {
        Box::pin(async move {
            let profile = ExecutionEnvironmentProfile::from_env_or_default();
            super::execution_environment::ensure_execution_environment(self, &profile, request)
                .await
        })
    }

    fn stop_execution_environment<'a>(
        &'a self,
        request: StopExecutionEnvironmentRequest,
    ) -> RunnerFuture<'a, anyhow::Result<StopExecutionEnvironmentAck>> {
        Box::pin(async move {
            let profile = ExecutionEnvironmentProfile::from_env_or_default();
            super::execution_environment::stop_execution_environment(self, &profile, request).await
        })
    }
}

impl DockerRunnerBackend {
    /// Run one filesystem-only preparation under the shared environment fence.
    pub(crate) async fn prepare_native_task_checkout(
        &self,
        request: PrepareNativeTaskRequest,
        seed: SeedBundle,
    ) -> anyhow::Result<NativeTaskPreparedResult> {
        let profile = ExecutionEnvironmentProfile::from_env_or_default();
        self.prepare_native_task_checkout_with_profile(&profile, request, seed)
            .await
    }

    pub(crate) async fn prepare_native_task_checkout_with_profile(
        &self,
        profile: &ExecutionEnvironmentProfile,
        request: PrepareNativeTaskRequest,
        seed: SeedBundle,
    ) -> anyhow::Result<NativeTaskPreparedResult> {
        if !self.native_tasks.root_configured() {
            return self.native_tasks.materialize(&request, &seed).await;
        }
        let (environment_id, generation) =
            protocol::validate_environment_owner(&request.owner).map_err(anyhow::Error::msg)?;
        let state_dir = profile.ensure_state_dir()?;
        let deadline = profile.operation_deadline()?;
        deadline.remaining_ms()?;
        let barrier = super::execution_environment::tombstone::TombstoneStore::open(
            state_dir,
            environment_id,
        )?;
        let environment_lock = self.environment_runner_lock(environment_id);
        let _environment_guard =
            tokio::time::timeout(deadline.remaining()?, environment_lock.lock())
                .await
                .map_err(|_| {
                    anyhow::anyhow!("timed out waiting for the native task preparation lock")
                })?;
        deadline.remaining_ms()?;
        let file_lock = super::execution_environment::file_lock::EnvFileLock::acquire(
            &barrier.lock_path(),
            super::execution_environment::file_lock::lock_holder_token(&self.manager_id),
        )?;

        let outcome = async {
            deadline.remaining_ms()?;
            let current = barrier.load()?;
            super::execution_environment::tombstone::ensure_generation_admissible(
                &current, generation,
            )?;
            barrier.save(super::execution_environment::tombstone::advance_high_water(
                current, generation,
            ))?;
            deadline.remaining_ms()?;
            let prepared = tokio::time::timeout(
                deadline.remaining()?,
                self.native_tasks.materialize(&request, &seed),
            )
            .await
            .map_err(|_| anyhow::anyhow!("timed out waiting for native task preparation"))??;
            deadline.remaining_ms()?;
            Ok(prepared)
        }
        .await;
        super::execution_environment::file_lock::release_and_finish(file_lock, outcome)
    }
}
