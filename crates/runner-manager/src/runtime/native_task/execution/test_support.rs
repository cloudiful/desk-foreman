use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use runner_protocol::{
    NativeTaskRuntimeIdentity, PrepareNativeTaskRequest, RunnerOwner, SourceSeedSnapshotFile,
    StartNativeTaskRequest, validate_seed_bundle,
};
use tempfile::TempDir;
use uuid::Uuid;

#[path = "test_support/fake_docker.rs"]
mod fake_docker;
#[path = "test_support/inspect.rs"]
mod inspect;
#[path = "test_support/stop_support.rs"]
mod stop_support;
use fake_docker::write_fake_docker;
use inspect::inspect_json;

use crate::config::{RunnerBackendKind, RunnerManagerConfig};
use crate::runtime::NativeTaskStore;
use crate::runtime::execution_environment::{
    ExecutionEnvironmentProfile,
    profile::{command::TrustedCommand, workdir::TrustedStateWorkdir},
    service::host_identity::UidSource,
};
use crate::runtime::{DockerRunnerBackend, RunnerLifecycleReporter, start_native_task};

pub(super) const ENVIRONMENT_ID: &str = "123e4567-e89b-12d3-a456-426614174000";
pub(super) const MANAGER_ID: &str = "manager-7";
pub(super) const HOST_ROOT: &str = "/daemon/native-tasks";

pub(super) struct Rig {
    _temp: TempDir,
    pub(super) backend: Arc<DockerRunnerBackend>,
    pub(super) profile: ExecutionEnvironmentProfile,
    pub(super) request: StartNativeTaskRequest,
    pub(super) prepare_request: PrepareNativeTaskRequest,
    pub(super) task_root: PathBuf,
    pub(super) task_tree: PathBuf,
    pub(super) state_dir: PathBuf,
    pub(super) host_tree: String,
    log: PathBuf,
    inspect_path: PathBuf,
    other_names_path: PathBuf,
    mode_path: PathBuf,
    marker: PathBuf,
}

impl Rig {
    pub(super) async fn prepared() -> Self {
        Self::build(false, false).await
    }

    pub(super) async fn prepared_with_local_root_alias() -> Self {
        Self::build(true, false).await
    }

    pub(super) async fn prepared_without_local_root() -> Self {
        Self::build(false, true).await
    }

    async fn build(local_root_alias: bool, disable_local_root: bool) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let task_root = temp.path().join("task-root");
        std::fs::create_dir(&task_root).expect("task root");
        set_mode(&task_root, 0o700);
        let state_dir = temp.path().join("state");
        std::fs::create_dir(&state_dir).expect("state root");
        set_mode(&state_dir, 0o700);

        let uid = UidSource::Proc.effective_uid().expect("manager uid");
        let gid = std::fs::metadata(&task_root).expect("root metadata").gid();
        let task_id = Uuid::new_v4();
        let owner = RunnerOwner::ExecutionEnvironment {
            environment_id: ENVIRONMENT_ID.to_string(),
            generation: 4,
        };
        let files = vec![SourceSeedSnapshotFile::new("README.md", "initial")];
        let seed = validate_seed_bundle(&files).expect("valid seed");
        let prepare_request = PrepareNativeTaskRequest {
            owner: owner.clone(),
            task_id,
            workspace_binding_id: 42,
            manifest_sha256: seed.manifest_sha256.clone(),
            file_count: seed.file_count,
            total_bytes: seed.total_bytes,
        };
        let store =
            NativeTaskStore::with_uid_source(Some(task_root.clone()), UidSource::Fixed(uid));
        store
            .materialize(&prepare_request, &seed)
            .await
            .expect("prepare task tree");
        let runtime_root = if disable_local_root {
            None
        } else if local_root_alias {
            let alias = temp.path().join("task-root-alias");
            std::os::unix::fs::symlink(&task_root, &alias).expect("task root alias");
            Some(alias)
        } else {
            Some(task_root.clone())
        };
        let store = if disable_local_root || local_root_alias {
            NativeTaskStore::with_uid_source(runtime_root.clone(), UidSource::Fixed(uid))
        } else {
            store
        };

        let task_tree = task_root
            .join(ENVIRONMENT_ID)
            .join(task_id.to_string())
            .join("tree");
        let host_tree = format!("{HOST_ROOT}/{ENVIRONMENT_ID}/{task_id}/tree");
        let request = StartNativeTaskRequest {
            owner,
            task: NativeTaskRuntimeIdentity {
                task_id,
                workspace_binding_id: prepare_request.workspace_binding_id,
                manifest_sha256: prepare_request.manifest_sha256.clone(),
                preparation_generation: 4,
            },
        };
        let container_name = request.owner.container_name();
        let log = temp.path().join("docker.log");
        let inspect_path = temp.path().join("inspect.json");
        let stopped_inspect_path = temp.path().join("inspect-exited.json");
        let other_names_path = temp.path().join("other-containers.txt");
        let mode_path = temp.path().join("docker.mode");
        let marker = temp.path().join("container.created");
        let barrier = state_dir
            .join(ENVIRONMENT_ID.replace('-', ""))
            .join("barrier.json");
        std::fs::write(&log, "").expect("docker log");
        std::fs::write(&mode_path, "ok").expect("docker mode");
        std::fs::write(&other_names_path, "").expect("other container names");
        let info = inspect_json(&request, &host_tree, uid, gid);
        std::fs::write(&inspect_path, info.to_string()).expect("inspect JSON");
        let mut stopped_info = info;
        stopped_info["State"]["Status"] = serde_json::json!("exited");
        std::fs::write(&stopped_inspect_path, stopped_info.to_string())
            .expect("stopped inspect JSON");
        let fake = write_fake_docker(
            temp.path(),
            &log,
            &inspect_path,
            &stopped_inspect_path,
            &other_names_path,
            &mode_path,
            &marker,
            &barrier,
            &container_name,
        );

        let mut config = crate::runtime::execution_environment::test_support::test_manager_config(
            &fake.to_string_lossy(),
        );
        config.manager_id = MANAGER_ID.to_string();
        config.backend = RunnerBackendKind::Docker;
        config.native_task_root = runtime_root;
        config.native_task_host_root = Some(PathBuf::from(HOST_ROOT));
        let config: Arc<tokio::sync::RwLock<RunnerManagerConfig>> =
            Arc::new(tokio::sync::RwLock::new(config));
        let backend = DockerRunnerBackend::new(
            config,
            MANAGER_ID.to_string(),
            RunnerLifecycleReporter::noop(),
            store,
        );
        let profile = ExecutionEnvironmentProfile {
            enabled: true,
            volume_uid: Some(uid),
            volume_gid: Some(gid),
            command: TrustedCommand::Configured(vec![
                "/opt/agent".to_string(),
                "serve --unchanged".to_string(),
            ]),
            state_workdir: TrustedStateWorkdir::Configured("/state/work".to_string()),
            state_dir: Some(state_dir.clone()),
            operation_deadline: std::time::Duration::from_secs(5),
            ..ExecutionEnvironmentProfile::default()
        };
        Self {
            _temp: temp,
            backend,
            profile,
            request,
            prepare_request,
            task_root,
            task_tree,
            state_dir,
            host_tree,
            log,
            inspect_path,
            other_names_path,
            mode_path,
            marker,
        }
    }

    pub(super) async fn start(&self) -> anyhow::Result<runner_protocol::StartNativeTaskResult> {
        start_native_task(&self.backend, &self.profile, self.request.clone()).await
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("set mode");
}
