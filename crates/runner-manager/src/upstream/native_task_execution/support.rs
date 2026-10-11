use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use reqwest::StatusCode;
use runner_protocol::{NativeTaskRuntimeIdentity, RunnerJob};
use serde_json::{Value, json};
use uuid::Uuid;

use super::super::{ProfiledDockerBackend, execute_job};
use super::{fake_docker::inspect_value, report::report_job};
use crate::{
    config::{RunnerBackendKind, RunnerManagerConfig},
    runtime::execution_environment::{
        ExecutionEnvironmentProfile,
        profile::{command::TrustedCommand, workdir::TrustedStateWorkdir},
        service::host_identity::UidSource,
        test_support::write_executable,
    },
    runtime::{DockerRunnerBackend, NativeTaskStore, RunnerBackend, RunnerLifecycleReporter},
    upstream::native_task_preparation::support::Rig as PreparationRig,
};

pub(crate) struct ManagerRig {
    pub(crate) fixture: PreparationRig,
    pub(super) backend: Arc<dyn RunnerBackend>,
    pub(crate) export_backend: Arc<dyn RunnerBackend>,
    pub(crate) fake: FakeDocker,
    pub(super) task_tree: PathBuf,
    pub(crate) env_id: Uuid,
    pub(crate) task_id: Uuid,
}

pub(crate) struct FakeDocker {
    pub(super) log_path: PathBuf,
    mode_path: PathBuf,
}

impl FakeDocker {
    fn new(
        dir: &Path,
        request: &runner_protocol::PrepareNativeTaskRequest,
        manager_id: i64,
        state_dir: &Path,
        host_root: &Path,
        uid: u32,
        gid: u32,
    ) -> Self {
        let (environment_id, preparation_generation) =
            runner_protocol::execution_environment::as_execution_environment(&request.owner)
                .expect("environment owner");
        let environment_uuid = Uuid::parse_str(environment_id).expect("environment UUID");
        let host_tree = host_root
            .join(environment_uuid.to_string())
            .join(request.task_id.to_string())
            .join("tree");
        let identity = NativeTaskRuntimeIdentity {
            task_id: request.task_id,
            workspace_binding_id: request.workspace_binding_id,
            manifest_sha256: request.manifest_sha256.clone(),
            preparation_generation,
        };
        let owner = request.owner.clone();
        let container_name = owner.container_name();
        let inspect_path = dir.join("task-inspect.json");
        let stopped_inspect_path = dir.join("task-inspect-exited.json");
        let other_names_path = dir.join("task-other-containers.txt");
        let mode_path = dir.join("task-docker-mode");
        let marker_path = dir.join("task-container.created");
        let log_path = dir.join("task-docker.log");
        let barrier_path = state_dir
            .join(environment_uuid.to_string().replace('-', ""))
            .join("barrier.json");
        std::fs::write(&log_path, "").expect("fake Docker log");
        std::fs::write(&other_names_path, "").expect("other container names");
        std::fs::write(&mode_path, "ok").expect("fake Docker mode");
        let running = inspect_value(
            &owner, &identity, &host_tree, manager_id, uid, gid, "running",
        );
        let exited = inspect_value(
            &owner, &identity, &host_tree, manager_id, uid, gid, "exited",
        );
        std::fs::write(&inspect_path, running.to_string()).expect("running inspect data");
        std::fs::write(&stopped_inspect_path, exited.to_string()).expect("stopped inspect data");
        let script = fake_docker_script(
            &log_path,
            &mode_path,
            &inspect_path,
            &stopped_inspect_path,
            &other_names_path,
            &marker_path,
            &barrier_path,
            &container_name,
        );
        write_executable(dir, "docker-task-fake", &script);
        Self {
            log_path,
            mode_path,
        }
    }

    pub(crate) fn set_mode(&self, mode: &str) {
        std::fs::write(&self.mode_path, mode).expect("set fake Docker mode");
    }

    pub(crate) fn log(&self) -> String {
        std::fs::read_to_string(&self.log_path).expect("read fake Docker log")
    }

    pub(super) fn stop_count(&self) -> usize {
        self.log()
            .lines()
            .filter(|line| line.starts_with("stop "))
            .count()
    }
}

impl ManagerRig {
    pub(crate) async fn prepared(files: &[(&str, &str)]) -> Self {
        let fixture = PreparationRig::start(files).await;
        let prepare_job = fixture.poll().await;
        assert_eq!(prepare_job.kind, "prepare_native_task");
        let prepare_request: runner_protocol::PrepareNativeTaskRequest =
            serde_json::from_value(prepare_job.payload.clone()).expect("preparation request");
        let temp_root = fixture.root.parent().expect("fixture temp root");
        let state_dir = temp_root.join("manager-state");
        std::fs::create_dir_all(&state_dir).expect("manager state root");
        set_private_dir(&state_dir);
        let host_root = PathBuf::from("/daemon/native-task-execution");
        let uid = UidSource::Proc.effective_uid().expect("manager uid");
        let gid = std::fs::metadata(&fixture.root)
            .expect("root metadata")
            .gid();
        let fake = FakeDocker::new(
            temp_root,
            &prepare_request,
            fixture.manager_id,
            &state_dir,
            &host_root,
            uid,
            gid,
        );
        let config: Arc<tokio::sync::RwLock<RunnerManagerConfig>> =
            crate::upstream::native_task_preparation::support::manager_config(
                Some(fixture.root.clone()),
                fixture.manager_id,
            );
        {
            let mut config = config.write().await;
            config.docker_cli = temp_root
                .join("docker-task-fake")
                .to_string_lossy()
                .into_owned();
            config.native_task_host_root = Some(host_root);
            config.backend = RunnerBackendKind::Docker;
        }
        let store =
            NativeTaskStore::with_uid_source(Some(fixture.root.clone()), UidSource::Fixed(uid));
        let docker = DockerRunnerBackend::new(
            config,
            fixture.manager_id.to_string(),
            RunnerLifecycleReporter::noop(),
            store,
        );
        let profile = ExecutionEnvironmentProfile {
            enabled: true,
            volume_uid: Some(uid),
            volume_gid: Some(gid),
            command: TrustedCommand::Configured(vec![
                "/opt/agent".to_string(),
                "serve".to_string(),
            ]),
            state_workdir: TrustedStateWorkdir::Configured("/state/work".to_string()),
            state_dir: Some(state_dir),
            operation_deadline: Duration::from_secs(10),
            ..ExecutionEnvironmentProfile::default()
        };
        let export_backend: Arc<dyn RunnerBackend> = docker.clone();
        let backend: Arc<dyn RunnerBackend> =
            Arc::new(ProfiledDockerBackend::new(Arc::clone(&docker), profile));
        let prepared = crate::upstream::native_task::prepare(
            &fixture.client,
            &fixture.base,
            &fixture.manager_token,
            &fixture.manager_id.to_string(),
            &prepare_job.job_id,
            prepare_request,
            backend.as_ref(),
        )
        .await
        .expect("fetch seed and materialize through guarded manager backend");
        let prepare_ack = report_job(
            &fixture,
            &prepare_job.job_id,
            true,
            Some(serde_json::to_value(prepared).expect("preparation result JSON")),
            None,
        )
        .await;
        assert_eq!(prepare_ack, StatusCode::NO_CONTENT);
        assert_eq!(fixture.task_state().await, "prepared");
        Self {
            env_id: fixture.env_id,
            task_id: fixture.task_id,
            task_tree: fixture.tree_dir(),
            fixture,
            backend,
            export_backend,
            fake,
        }
    }

    pub(crate) async fn schedule(
        &self,
        operation: &str,
        generation: i64,
        token: &str,
    ) -> reqwest::Response {
        self.fixture
            .client
            .post(format!(
                "{}/api/internal/execution-environments/{}/tasks/{}/{operation}",
                self.fixture.base, self.env_id, self.task_id
            ))
            .bearer_auth(&self.fixture.application_token)
            .header("X-DF-External-User-Id", "prep-user")
            .json(&json!({
                "expected_generation": generation,
                "operation_token": token,
            }))
            .send()
            .await
            .expect("schedule native task operation")
    }

    pub(crate) async fn poll(&self, expected_kind: &str) -> RunnerJob {
        let job = self.fixture.poll().await;
        assert_eq!(job.kind, expected_kind);
        job
    }

    pub(crate) async fn execute(&self, job: &RunnerJob) -> anyhow::Result<Value> {
        execute_job(self.backend.as_ref(), job).await
    }

    pub(crate) async fn report_success(&self, job: &RunnerJob, result: Value) -> StatusCode {
        report_job(&self.fixture, &job.job_id, true, Some(result), None).await
    }

    pub(crate) async fn report_failure(&self, job: &RunnerJob, error: &str) -> StatusCode {
        report_job(&self.fixture, &job.job_id, false, None, Some(error)).await
    }

    pub(crate) fn tree_file(&self, relative: &str) -> PathBuf {
        self.task_tree.join(relative)
    }
}

fn fake_docker_script(
    log: &Path,
    mode: &Path,
    inspect: &Path,
    stopped_inspect: &Path,
    other_names: &Path,
    marker: &Path,
    barrier: &Path,
    container_name: &str,
) -> String {
    let quote = |path: &Path| shell_quote(path.to_str().expect("UTF-8 test path"));
    format!(
        "#!/bin/bash\necho \"$@\" >> {}\nmode=$(cat {})\ncheck_barrier() {{ if [ \"$mode\" = require-stop-barrier ] && ! grep -Eq '\"stopped_generation\":[1-9]' {}; then echo 'missing stop barrier' >&2; exit 1; fi; }}\ncase \"$1\" in\n  volume) exit 0 ;;\n  ps) check_barrier; cat {}; if [ -f {} ]; then echo {}; fi; exit 0 ;;\n  inspect) check_barrier; if [ \"$2\" = {} ] && [ -f {} ]; then cat {}; exit 0; fi; if [ \"$2\" != {} ] && grep -Fxq \"$2\" {}; then cat {}; exit 0; fi; echo 'Error: No such container' >&2; exit 1 ;;\n  run) if [ \"$mode\" = fail-run ]; then exit 1; fi; touch {}; exit 0 ;;\n  exec) if [ \"$mode\" = fail-task-probe ] && [[ \"$*\" == *\"--workdir /workspace\"* ]]; then exit 1; fi; exit 0 ;;\n  stop) check_barrier; if [ \"$mode\" = stop-stuck ]; then exit 1; fi; cp {} {}; exit 0 ;;\n  rm) check_barrier; if [ \"$mode\" = stop-stuck ]; then exit 1; fi; rm -f {}; exit 0 ;;\n  *) exit 0 ;;\nesac\n",
        quote(log),
        quote(mode),
        quote(barrier),
        quote(other_names),
        quote(marker),
        shell_quote(container_name),
        shell_quote(container_name),
        quote(marker),
        quote(inspect),
        shell_quote(container_name),
        quote(other_names),
        quote(inspect),
        quote(marker),
        quote(stopped_inspect),
        quote(inspect),
        quote(marker),
    )
}

fn shell_quote(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "'\\''"))
}

#[cfg(unix)]
fn set_private_dir(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).expect("private mode");
}
