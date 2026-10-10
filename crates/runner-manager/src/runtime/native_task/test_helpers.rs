//! Shared helpers for the native-task filesystem tests (issue 753 P2b).
//!
//! Split so each test module stays inside the size budget.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use runner_protocol::{
    PrepareNativeTaskRequest, RunnerOwner, SourceSeedSnapshotFile, validate_seed_bundle,
};
use uuid::Uuid;

use super::store::NativeTaskStore;
use crate::runtime::execution_environment::service::host_identity::UidSource;

pub(super) fn current_uid() -> u32 {
    UidSource::Proc.effective_uid().expect("effective uid")
}

pub(super) fn root() -> PathBuf {
    let dir = tempfile::tempdir().expect("tempdir").keep();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).expect("mode");
    dir
}

pub(super) fn request(files: &[(&str, &str)]) -> PrepareNativeTaskRequest {
    let entries: Vec<SourceSeedSnapshotFile> = files
        .iter()
        .map(|(path, content)| SourceSeedSnapshotFile::new(*path, *content))
        .collect();
    let bundle = validate_seed_bundle(&entries).expect("bundle");
    PrepareNativeTaskRequest {
        owner: RunnerOwner::ExecutionEnvironment {
            environment_id: Uuid::new_v4().to_string(),
            generation: 4,
        },
        task_id: Uuid::new_v4(),
        workspace_binding_id: 7,
        manifest_sha256: bundle.manifest_sha256,
        file_count: bundle.file_count,
        total_bytes: bundle.total_bytes,
    }
}

pub(super) fn bundle_of(files: &[(&str, &str)]) -> runner_protocol::SeedBundle {
    let entries: Vec<SourceSeedSnapshotFile> = files
        .iter()
        .map(|(path, content)| SourceSeedSnapshotFile::new(*path, *content))
        .collect();
    validate_seed_bundle(&entries).expect("bundle")
}

pub(super) fn store(root: &Path) -> NativeTaskStore {
    NativeTaskStore::with_uid_source(Some(root.to_path_buf()), UidSource::Fixed(current_uid()))
}

pub(super) fn layout_of(root: &Path, request: &PrepareNativeTaskRequest) -> PathBuf {
    let (env, _) = runner_protocol::execution_environment::as_execution_environment(&request.owner)
        .expect("environment owner");
    root.join(env).join(request.task_id.to_string())
}
