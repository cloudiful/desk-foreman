use crate::runtime::execution_environment::{
    ExecutionEnvironmentProfile,
    service::host_identity::UidSource,
    tombstone::{EnvironmentBarrier, TombstoneStore},
};
use crate::runtime::{DockerRunnerBackend, NativeTaskStore, RunnerLifecycleReporter};
use runner_protocol::*;
use std::{fs, os::unix::fs::PermissionsExt, sync::Arc};

struct Rig {
    _temp: tempfile::TempDir,
    backend: Arc<DockerRunnerBackend>,
    profile: ExecutionEnvironmentProfile,
    request: ExportNativeTaskRequest,
    tree: std::path::PathBuf,
    barrier: TombstoneStore,
}

impl Rig {
    async fn prepared() -> Self {
        let cache = std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
            .join(".cache/opencode/df-native-task-export/capture-tests");
        fs::create_dir_all(&cache).unwrap();
        let temp = tempfile::tempdir_in(cache).unwrap();
        let root = temp.path().join("root");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let state = temp.path().join("state");
        fs::create_dir(&state).unwrap();
        let uid = UidSource::Proc.effective_uid().unwrap();
        let store = NativeTaskStore::with_uid_source(Some(root.clone()), UidSource::Fixed(uid));
        let environment = uuid::Uuid::new_v4().to_string();
        let task_id = uuid::Uuid::new_v4();
        let owner = RunnerOwner::ExecutionEnvironment {
            environment_id: environment.clone(),
            generation: 4,
        };
        let seed =
            validate_seed_bundle(&[SourceSeedSnapshotFile::new("original", "seed")]).unwrap();
        store
            .materialize(
                &PrepareNativeTaskRequest {
                    owner: owner.clone(),
                    task_id,
                    workspace_binding_id: 42,
                    manifest_sha256: seed.manifest_sha256.clone(),
                    file_count: seed.file_count,
                    total_bytes: seed.total_bytes,
                },
                &seed,
            )
            .await
            .unwrap();
        let selection = validate_native_task_export_selection(&[
            "original".into(),
            "added".into(),
            "deleted".into(),
            "empty".into(),
        ])
        .unwrap();
        let request = ExportNativeTaskRequest {
            owner,
            task: NativeTaskRuntimeIdentity {
                task_id,
                workspace_binding_id: 42,
                manifest_sha256: seed.manifest_sha256,
                preparation_generation: 4,
            },
            stop_job_id: uuid::Uuid::new_v4(),
            selection_sha256: selection.selection_sha256,
            paths: selection.paths,
        };
        let mut config = crate::runtime::execution_environment::test_support::test_manager_config(
            "/never-call-docker-for-export",
        );
        config.native_task_root = Some(root.clone());
        config.native_task_host_root = None;
        let backend = DockerRunnerBackend::new(
            Arc::new(tokio::sync::RwLock::new(config)),
            "manager-7".into(),
            RunnerLifecycleReporter::noop(),
            store,
        );
        let profile = ExecutionEnvironmentProfile {
            state_dir: Some(state.clone()),
            ..Default::default()
        };
        let barrier = TombstoneStore::open(&state, &environment).unwrap();
        let tree = root
            .join(environment)
            .join(task_id.to_string())
            .join("tree");
        Self {
            _temp: temp,
            backend,
            profile,
            request,
            tree,
            barrier,
        }
    }

    fn stop(&self) {
        self.barrier
            .save(EnvironmentBarrier {
                high_water: 9,
                stopped_generation: Some(4),
            })
            .unwrap();
    }
}

#[tokio::test]
async fn native_task_export_capture_retains_edits_with_successor_high_water() {
    let rig = Rig::prepared().await;
    rig.stop();
    for (name, text) in [("original", "edited"), ("added", "new"), ("empty", "")] {
        fs::write(rig.tree.join(name), text).unwrap();
        fs::set_permissions(rig.tree.join(name), fs::Permissions::from_mode(0o644)).unwrap();
    }
    fs::write(rig.tree.join("unselected"), [0, 255]).unwrap();
    let journal = rig.tree.parent().unwrap().join("journal.json");
    let before = fs::read(&journal).unwrap();
    let barrier_before = fs::read(rig.barrier.barrier_path()).unwrap();
    let output = super::export_native_task(&rig.backend, &rig.profile, rig.request.clone())
        .await
        .unwrap();
    assert_eq!(output.missing_paths, vec!["deleted"]);
    assert_eq!(output.files.len(), 3);
    assert_eq!(
        output
            .files
            .iter()
            .find(|file| file.path == "original")
            .unwrap()
            .content,
        "edited"
    );
    assert_eq!(fs::read(journal).unwrap(), before);
    assert_eq!(
        fs::read(rig.barrier.barrier_path()).unwrap(),
        barrier_before
    );
    assert!(!rig.barrier.lock_path().exists());
    assert_eq!(
        super::export_native_task(&rig.backend, &rig.profile, rig.request.clone())
            .await
            .unwrap(),
        output
    );
}

#[tokio::test]
async fn native_task_export_capture_rejects_absent_journal_task_and_unsafe_selection() {
    for boundary in ["journal", "task", "symlink", "preparing"] {
        let rig = Rig::prepared().await;
        rig.stop();
        let task = rig.tree.parent().unwrap();
        match boundary {
            "journal" => {
                fs::rename(task.join("journal.json"), task.join("journal.retained")).unwrap()
            }
            "task" => fs::rename(task, task.with_extension("retained")).unwrap(),
            "preparing" => {
                let path = task.join("journal.json");
                let mut journal: serde_json::Value =
                    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                journal["phase"] = serde_json::json!("preparing");
                fs::write(path, serde_json::to_vec(&journal).unwrap()).unwrap();
            }
            _ => {
                fs::remove_file(rig.tree.join("original")).unwrap();
                std::os::unix::fs::symlink("missing", rig.tree.join("original")).unwrap();
            }
        }
        let before = fs::read(rig.barrier.barrier_path()).unwrap();
        assert!(
            super::export_native_task(&rig.backend, &rig.profile, rig.request.clone())
                .await
                .is_err()
        );
        assert_eq!(fs::read(rig.barrier.barrier_path()).unwrap(), before);
        assert!(!rig.barrier.lock_path().exists());
    }
}

#[tokio::test]
async fn native_task_export_capture_all_missing_selection_preserves_checkout() {
    let rig = Rig::prepared().await;
    rig.stop();
    let selection = validate_native_task_export_selection(&[
        "absent-parent/child.txt".into(),
        "absent-leaf.txt".into(),
    ])
    .unwrap();
    let mut request = rig.request.clone();
    request.paths = selection.paths.clone();
    request.selection_sha256 = selection.selection_sha256.clone();
    let journal = rig.tree.parent().unwrap().join("journal.json");
    let journal_before = fs::read(&journal).unwrap();
    let barrier_before = fs::read(rig.barrier.barrier_path()).unwrap();
    let output = super::export_native_task(&rig.backend, &rig.profile, request.clone())
        .await
        .unwrap();
    assert!(output.files.is_empty());
    assert_eq!(output.missing_paths, selection.paths);
    assert_eq!(
        output.output_manifest_sha256,
        native_task_export_output_manifest_sha256(&selection, &[], &selection.paths).unwrap()
    );
    assert_eq!(
        validate_native_task_export_content(&request, &output)
            .unwrap()
            .total_bytes,
        0
    );
    assert_eq!(fs::read(rig.tree.join("original")).unwrap(), b"seed");
    assert!(!rig.tree.join("absent-parent").exists());
    assert_eq!(fs::read(journal).unwrap(), journal_before);
    assert_eq!(
        fs::read(rig.barrier.barrier_path()).unwrap(),
        barrier_before
    );
    assert!(!rig.barrier.lock_path().exists());
}

#[tokio::test]
async fn native_task_export_capture_requires_barrier_and_matching_identity() {
    let rig = Rig::prepared().await;
    assert!(
        super::export_native_task(&rig.backend, &rig.profile, rig.request.clone())
            .await
            .is_err()
    );
    rig.stop();
    let mut wrong = rig.request.clone();
    wrong.task.workspace_binding_id += 1;
    assert!(
        super::export_native_task(&rig.backend, &rig.profile, wrong)
            .await
            .is_err()
    );
    fs::rename(&rig.tree, rig.tree.with_file_name("retained")).unwrap();
    assert!(
        super::export_native_task(&rig.backend, &rig.profile, rig.request.clone())
            .await
            .is_err()
    );
    assert!(!rig.tree.exists());
    assert!(!rig.barrier.lock_path().exists());
}
