use std::{path::PathBuf, sync::Arc};

use crate::{
    runtime::{
        DockerRunnerBackend, NativeTaskStore, RunnerBackend, RunnerLifecycleReporter,
        execution_environment::{ExecutionEnvironmentProfile, service::host_identity::UidSource},
    },
    upstream::dispatch::ProfiledDockerBackend,
};

pub(super) fn build(
    root: Option<PathBuf>,
    manager_id: i64,
    state_parent: &std::path::Path,
) -> Arc<dyn RunnerBackend> {
    use std::os::unix::fs::PermissionsExt;

    let state_dir = state_parent.join("manager-state");
    std::fs::create_dir_all(&state_dir).expect("manager state root");
    std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o700))
        .expect("manager state mode");
    let store =
        NativeTaskStore::with_uid_source(root.clone(), UidSource::Fixed(super::current_uid()));
    let backend = DockerRunnerBackend::new(
        super::manager_config(root, manager_id),
        manager_id.to_string(),
        RunnerLifecycleReporter::noop(),
        store,
    );
    Arc::new(ProfiledDockerBackend::new(
        backend,
        ExecutionEnvironmentProfile {
            state_dir: Some(state_dir),
            ..ExecutionEnvironmentProfile::default()
        },
    ))
}
