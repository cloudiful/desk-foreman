use std::{path::PathBuf, sync::Arc};

use runner_protocol::StopNativeTaskRequest;
use serde_json::Value;

use super::{
    DockerRunnerBackend, ENVIRONMENT_ID, ExecutionEnvironmentProfile, MANAGER_ID, NativeTaskStore,
    Rig, RunnerLifecycleReporter, UidSource,
};

impl Rig {
    pub(in crate::runtime::native_task::execution::tests) async fn stop(
        &self,
    ) -> anyhow::Result<runner_protocol::StopNativeTaskResult> {
        self.stop_with_profile(&self.profile).await
    }

    pub(in crate::runtime::native_task::execution::tests) async fn stop_with_profile(
        &self,
        profile: &ExecutionEnvironmentProfile,
    ) -> anyhow::Result<runner_protocol::StopNativeTaskResult> {
        crate::runtime::stop_native_task(&self.backend, profile, self.stop_request()).await
    }

    pub(in crate::runtime::native_task::execution::tests) fn stop_request(
        &self,
    ) -> StopNativeTaskRequest {
        StopNativeTaskRequest {
            owner: self.request.owner.clone(),
            task: self.request.task.clone(),
        }
    }

    pub(in crate::runtime::native_task::execution::tests) async fn reopened_backend(
        &self,
    ) -> Arc<DockerRunnerBackend> {
        let config = self.backend.config.read().await.clone();
        let store = NativeTaskStore::with_uid_source(
            config.native_task_root.clone(),
            UidSource::Fixed(UidSource::Proc.effective_uid().expect("manager uid")),
        );
        DockerRunnerBackend::new(
            Arc::new(tokio::sync::RwLock::new(config)),
            MANAGER_ID.to_string(),
            RunnerLifecycleReporter::noop(),
            store,
        )
    }

    pub(in crate::runtime::native_task::execution::tests) fn set_profile_uid(&mut self, uid: u32) {
        self.profile.volume_uid = Some(uid);
    }

    pub(in crate::runtime::native_task::execution::tests) async fn configure_host_root(
        &self,
        root: Option<PathBuf>,
    ) {
        self.backend.config.write().await.native_task_host_root = root;
    }

    pub(in crate::runtime::native_task::execution::tests) fn set_mode(&self, mode: &str) {
        std::fs::write(&self.mode_path, mode).expect("write fake mode");
    }

    pub(in crate::runtime::native_task::execution::tests) fn set_operation_deadline(
        &mut self,
        deadline: std::time::Duration,
    ) {
        self.profile.operation_deadline = deadline;
    }

    pub(in crate::runtime::native_task::execution::tests) fn set_inspect(&self, info: Value) {
        std::fs::write(&self.inspect_path, info.to_string()).expect("write fake inspect");
    }

    pub(in crate::runtime::native_task::execution::tests) fn mark_container_exists(&self) {
        std::fs::write(&self.marker, "created").expect("mark container");
    }

    pub(in crate::runtime::native_task::execution::tests) fn set_other_container_names(
        &self,
        names: &[&str],
    ) {
        let contents = names.join("\n");
        std::fs::write(&self.other_names_path, contents).expect("write container names");
    }

    pub(in crate::runtime::native_task::execution::tests) fn log(&self) -> String {
        std::fs::read_to_string(&self.log).expect("read Docker log")
    }

    pub(in crate::runtime::native_task::execution::tests) fn run_count(&self) -> usize {
        self.log()
            .lines()
            .filter(|line| line.starts_with("run "))
            .count()
    }

    pub(in crate::runtime::native_task::execution::tests) fn stop_count(&self) -> usize {
        self.log()
            .lines()
            .filter(|line| line.starts_with("stop "))
            .count()
    }

    pub(in crate::runtime::native_task::execution::tests) fn inspect_value(&self) -> Value {
        serde_json::from_slice(&std::fs::read(&self.inspect_path).expect("read fake inspect JSON"))
            .expect("fake inspect value")
    }

    pub(in crate::runtime::native_task::execution::tests) fn stopped_barrier_path(
        &self,
    ) -> PathBuf {
        let environment_dir = ENVIRONMENT_ID.replace('-', "");
        self.state_dir.join(environment_dir).join("barrier.json")
    }
}
