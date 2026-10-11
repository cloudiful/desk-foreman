use anyhow::bail;
use runner_protocol::{
    ExportNativeTaskRequest, RunnerJob, StartNativeTaskRequest, StopNativeTaskRequest,
};
use serde_json::Value;

use crate::runtime::RunnerBackend;

#[cfg(test)]
#[path = "native_task_execution.rs"]
pub(crate) mod native_task_execution;

#[cfg(test)]
pub(crate) struct ProfiledDockerBackend {
    backend: std::sync::Arc<crate::runtime::DockerRunnerBackend>,
    profile: crate::runtime::execution_environment::ExecutionEnvironmentProfile,
}

#[cfg(test)]
impl ProfiledDockerBackend {
    pub(crate) fn new(
        backend: std::sync::Arc<crate::runtime::DockerRunnerBackend>,
        profile: crate::runtime::execution_environment::ExecutionEnvironmentProfile,
    ) -> Self {
        Self { backend, profile }
    }
}

#[cfg(test)]
impl RunnerBackend for ProfiledDockerBackend {
    fn reconcile<'a>(&'a self) -> desk_foreman::runner::RunnerFuture<'a, anyhow::Result<()>> {
        self.backend.reconcile()
    }

    fn prepare_shell_spawn<'a>(
        &'a self,
        request: runner_protocol::RunnerShellRequest,
    ) -> desk_foreman::runner::RunnerFuture<'a, anyhow::Result<crate::runtime::ProcessSpawnTarget>>
    {
        self.backend.prepare_shell_spawn(request)
    }

    fn run_command<'a>(
        &'a self,
        request: runner_protocol::RunnerCommandRequest,
    ) -> desk_foreman::runner::RunnerFuture<'a, anyhow::Result<runner_protocol::CommandOutput>>
    {
        self.backend.run_command(request)
    }

    fn cleanup_runner_owner<'a>(
        &'a self,
        owner: runner_protocol::RunnerOwner,
    ) -> desk_foreman::runner::RunnerFuture<'a, anyhow::Result<()>> {
        self.backend.cleanup_runner_owner(owner)
    }

    fn prepare_native_task<'a>(
        &'a self,
        request: runner_protocol::PrepareNativeTaskRequest,
        seed: runner_protocol::SeedBundle,
    ) -> desk_foreman::runner::RunnerFuture<
        'a,
        anyhow::Result<runner_protocol::NativeTaskPreparedResult>,
    > {
        Box::pin(async move {
            self.backend
                .prepare_native_task_checkout_with_profile(&self.profile, request, seed)
                .await
        })
    }

    fn start_native_task<'a>(
        &'a self,
        request: runner_protocol::StartNativeTaskRequest,
    ) -> desk_foreman::runner::RunnerFuture<
        'a,
        anyhow::Result<runner_protocol::StartNativeTaskResult>,
    > {
        Box::pin(async move {
            crate::runtime::start_native_task(&self.backend, &self.profile, request).await
        })
    }

    fn stop_native_task<'a>(
        &'a self,
        request: runner_protocol::StopNativeTaskRequest,
    ) -> desk_foreman::runner::RunnerFuture<'a, anyhow::Result<runner_protocol::StopNativeTaskResult>>
    {
        Box::pin(async move {
            crate::runtime::stop_native_task(&self.backend, &self.profile, request).await
        })
    }

    fn begin_shell_operation(&self, owner: &runner_protocol::RunnerOwner) {
        self.backend.begin_shell_operation(owner);
    }

    fn end_shell_operation(&self, owner: &runner_protocol::RunnerOwner) {
        self.backend.end_shell_operation(owner);
    }

    fn touch_activity(&self, owner: &runner_protocol::RunnerOwner) {
        self.backend.touch_activity(owner);
    }
}

/// Execute one typed native-task execution job through the manager's shared backend.
pub(super) async fn execute_job(
    backend: &dyn RunnerBackend,
    job: &RunnerJob,
) -> anyhow::Result<Value> {
    match job.kind.as_str() {
        "start_native_task" => {
            let request: StartNativeTaskRequest = serde_json::from_value(job.payload.clone())?;
            Ok(serde_json::to_value(
                backend.start_native_task(request).await?,
            )?)
        }
        "stop_native_task" => {
            let request: StopNativeTaskRequest = serde_json::from_value(job.payload.clone())?;
            Ok(serde_json::to_value(
                backend.stop_native_task(request).await?,
            )?)
        }
        "export_native_task" => {
            let request: ExportNativeTaskRequest = serde_json::from_value(job.payload.clone())?;
            backend.export_native_task(request).await?;
            bail!("native task export requires the bounded upload dispatcher")
        }
        _ => bail!("native execution dispatcher received an unsupported job kind"),
    }
}

#[cfg(test)]
mod tests {
    use desk_foreman::runner::RunnerFuture;
    use runner_protocol::{
        CommandOutput, EnsureExecutionEnvironmentAck, NativeTaskRuntimeIdentity,
        RunnerCommandRequest, RunnerJob, RunnerOwner, RunnerShellRequest, StartNativeTaskRequest,
        StartNativeTaskResult,
    };
    use serde_json::json;
    use uuid::Uuid;

    use super::execute_job;
    use crate::runtime::{ProcessSpawnTarget, RunnerBackend};

    struct StartBackend;

    impl RunnerBackend for StartBackend {
        fn reconcile<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn prepare_shell_spawn<'a>(
            &'a self,
            _request: RunnerShellRequest,
        ) -> RunnerFuture<'a, anyhow::Result<ProcessSpawnTarget>> {
            Box::pin(async { anyhow::bail!("unused test backend path") })
        }

        fn run_command<'a>(
            &'a self,
            _request: RunnerCommandRequest,
        ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
            Box::pin(async { anyhow::bail!("unused test backend path") })
        }

        fn cleanup_runner_owner<'a>(
            &'a self,
            _owner: RunnerOwner,
        ) -> RunnerFuture<'a, anyhow::Result<()>> {
            Box::pin(async { Ok(()) })
        }

        fn start_native_task<'a>(
            &'a self,
            request: StartNativeTaskRequest,
        ) -> RunnerFuture<'a, anyhow::Result<StartNativeTaskResult>> {
            Box::pin(async move {
                Ok(StartNativeTaskResult {
                    task: request.task,
                    environment: EnsureExecutionEnvironmentAck {
                        owner: request.owner,
                        container_name: "test-native-task".to_string(),
                        state: "running".to_string(),
                    },
                })
            })
        }
    }

    fn request() -> StartNativeTaskRequest {
        StartNativeTaskRequest {
            owner: RunnerOwner::ExecutionEnvironment {
                environment_id: "123e4567-e89b-12d3-a456-426614174000".to_string(),
                generation: 4,
            },
            task: NativeTaskRuntimeIdentity {
                task_id: Uuid::from_u128(7),
                workspace_binding_id: 42,
                manifest_sha256: "a".repeat(64),
                preparation_generation: 4,
            },
        }
    }

    #[tokio::test]
    async fn start_dispatch_uses_the_typed_request_and_shared_backend() {
        let request = request();
        let job = RunnerJob {
            job_id: "envjob:123e4567-e89b-12d3-a456-426614174001".to_string(),
            kind: "start_native_task".to_string(),
            payload: serde_json::to_value(&request).expect("request JSON"),
        };
        let result = execute_job(&StartBackend, &job).await.expect("start route");
        assert_eq!(
            result["task"],
            serde_json::to_value(request.task).expect("task")
        );
        assert_eq!(result["environment"]["state"], "running");
    }

    #[tokio::test]
    async fn native_stop_dispatch_stays_fail_closed_by_default() {
        let start = request();
        let stop = runner_protocol::StopNativeTaskRequest {
            owner: start.owner,
            task: start.task,
        };
        let job = RunnerJob {
            job_id: "envjob:123e4567-e89b-12d3-a456-426614174001".to_string(),
            kind: "stop_native_task".to_string(),
            payload: serde_json::to_value(stop).expect("stop JSON"),
        };
        let backend = crate::runtime::DirectRunnerBackend::new();
        assert!(execute_job(backend.as_ref(), &job).await.is_err());
    }

    #[tokio::test]
    async fn native_export_dispatch_stays_fail_closed_by_default() {
        let start = request();
        let selection =
            runner_protocol::validate_native_task_export_selection(&["README.md".to_string()])
                .unwrap();
        let job = RunnerJob {
            job_id: "envjob:123e4567-e89b-12d3-a456-426614174001".to_string(),
            kind: "export_native_task".to_string(),
            payload: serde_json::to_value(runner_protocol::ExportNativeTaskRequest {
                owner: start.owner,
                task: start.task,
                stop_job_id: Uuid::from_u128(8),
                selection_sha256: selection.selection_sha256,
                paths: selection.paths,
            })
            .unwrap(),
        };
        let backend = crate::runtime::DirectRunnerBackend::new();
        let error = execute_job(backend.as_ref(), &job).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("native task export is not configured")
        );
    }

    #[tokio::test]
    async fn start_dispatch_rejects_unowned_configuration_fields() {
        let mut payload = serde_json::to_value(request()).expect("request JSON");
        payload["task_host_root"] = json!("/untrusted");
        let job = RunnerJob {
            job_id: "envjob:123e4567-e89b-12d3-a456-426614174001".to_string(),
            kind: "start_native_task".to_string(),
            payload,
        };
        assert!(execute_job(&StartBackend, &job).await.is_err());
    }
}
