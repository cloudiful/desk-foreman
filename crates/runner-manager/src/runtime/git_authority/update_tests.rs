use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use runner_protocol::{GitJobSpec, GitObjectFormat, GitReceiptPhase, RunnerOwner};
use tokio::sync::watch;
use uuid::Uuid;

use super::clone_bundle::{GitBundleIdentity, VerifiedGitBundle};
use super::clone_plan::{
    GitCloneCommand, GitCloneConfig, GitCloneExecutor, GitCloneLimits, GitPlan, GitPlanFuture,
    GitPlanOutcome,
};
use super::clone_volume::GitVolumePlan;
use super::clone_workspace::CloneWorkspace;
use super::receipt::read_receipt;
use super::update::{UpdateFailureCode, update_verified_bundle};
use crate::config::{RunnerBackendKind, RunnerManagerConfig};
use crate::runtime::direct::DirectRunnerBackend;
use crate::runtime::local_service::LocalRunnerService;

const BASE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HEAD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const OTHER: &str = "cccccccccccccccccccccccccccccccccccccccc";

#[derive(Default)]
struct WorkspaceState {
    head: String,
    branch: String,
    clean: bool,
    ancestor: bool,
    fail_read_tree: bool,
    fail_rollback: bool,
    fail_cas: bool,
    external_move_on_cas: bool,
    read_tree_calls: usize,
}

struct FakeExecutor {
    state: Arc<Mutex<WorkspaceState>>,
}

impl GitCloneExecutor for FakeExecutor {
    fn run_plan<'a>(
        &'a self,
        plan: &'a GitPlan,
        _cancel: Option<watch::Receiver<bool>>,
    ) -> GitPlanFuture<'a> {
        Box::pin(async move {
            let mut state = self.state.lock().expect("state");
            let outcome = match &plan.command {
                GitCloneCommand::WorkspaceHead => {
                    outcome(0, format!("{}\n", state.head).as_bytes())
                }
                GitCloneCommand::WorkspaceBranch => {
                    outcome(0, format!("{}\n", state.branch).as_bytes())
                }
                GitCloneCommand::WorkspaceStatus => {
                    if state.clean {
                        outcome(0, b"")
                    } else {
                        outcome(0, b" M dirty\n")
                    }
                }
                GitCloneCommand::WorkspaceImportBundle => outcome(0, b""),
                GitCloneCommand::WorkspaceCommitExists { .. } => outcome(0, b""),
                GitCloneCommand::WorkspaceIsAncestor { .. } => {
                    outcome(if state.ancestor { 0 } else { 1 }, b"")
                }
                GitCloneCommand::WorkspaceReadTree { old, new } => {
                    state.read_tree_calls += 1;
                    let reverse = old == HEAD;
                    if state.fail_read_tree && !reverse || state.fail_rollback && reverse {
                        return Ok(outcome(1, b"read-tree failed\n"));
                    }
                    let _ = new;
                    outcome(0, b"")
                }
                GitCloneCommand::WorkspaceUpdateRef { old, new, .. } => {
                    if state.external_move_on_cas {
                        state.head = OTHER.to_owned();
                        return Ok(outcome(1, b"cas failed\n"));
                    }
                    if state.external_move_on_cas {
                        state.head = OTHER.to_owned();
                        return Ok(outcome(1, b"cas failed\n"));
                    }
                    if state.fail_cas || state.head != *old {
                        return Ok(outcome(1, b"cas failed\n"));
                    }
                    state.head = new.clone();
                    outcome(0, b"")
                }
                _ => outcome(0, b""),
            };
            Ok(outcome)
        })
    }

    fn create_volume<'a>(&'a self, _plan: &'a GitVolumePlan) -> GitPlanFuture<'a> {
        Box::pin(async { Ok(outcome(0, b"")) })
    }

    fn remove_volume<'a>(&'a self, _plan: &'a GitVolumePlan) -> GitPlanFuture<'a> {
        Box::pin(async { Ok(outcome(0, b"")) })
    }
}

fn outcome(exit_code: i32, stdout: &[u8]) -> GitPlanOutcome {
    GitPlanOutcome {
        exit_code,
        stdout: stdout.to_vec(),
        stderr: Vec::new(),
    }
}

fn private_dir() -> tempfile::TempDir {
    let base = std::env::var_os("HOME").expect("home directory");
    let temp = tempfile::Builder::new().tempdir_in(&base).expect("temp");
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).expect("mode");
    temp
}

fn clone_config(root: &std::path::Path) -> GitCloneConfig {
    let metadata = std::fs::metadata(root).expect("root metadata");
    use std::os::unix::fs::MetadataExt;
    GitCloneConfig {
        private_root: root.to_path_buf(),
        host_private_root: root.to_path_buf(),
        limits: GitCloneLimits {
            image: format!("helper@sha256:{}", "a".repeat(64)),
            network: "df-git-net".to_owned(),
            broker_host: "broker.example.invalid".to_owned(),
            uid: metadata.uid(),
            gid: metadata.gid(),
            fetch_volume_bytes: 1024 * 1024,
            bundle_bytes: 1024 * 1024,
            logical_object_bytes: 1024 * 1024,
            checkout_bytes: 1024 * 1024,
            tree_entries: 16,
            deadline: Duration::from_secs(10),
            memory_limit: "256m".to_owned(),
            cpu_limit: "1".to_owned(),
            pids_limit: 16,
        },
    }
}

fn config(root: &std::path::Path) -> RunnerManagerConfig {
    RunnerManagerConfig {
        control_plane_url: None,
        manager_id: "1".to_owned(),
        bind_addr: "127.0.0.1:0".to_owned(),
        auth_token: "token".to_owned(),
        backend: RunnerBackendKind::Docker,
        workspace_root: root.to_path_buf(),
        host_workspace_root: root.to_path_buf(),
        image: "runner:latest".to_owned(),
        git_image: Some(format!("helper@sha256:{}", "a".repeat(64))),
        git_private_root: Some(root.to_path_buf()),
        git_host_private_root: Some(root.to_path_buf()),
        git_network: Some("df-git-net".to_owned()),
        git_broker_host: Some("broker.example.invalid".to_owned()),
        git_max_fetch_volume_bytes: Some(1024 * 1024),
        git_max_bundle_bytes: Some(1024 * 1024),
        git_max_logical_object_bytes: Some(1024 * 1024),
        git_max_checkout_bytes: Some(1024 * 1024),
        git_max_tree_entries: Some(16),
        workdir: "/workspace".to_owned(),
        network_enabled: false,
        max_output_bytes: 262_144,
        max_timeout_ms: 60_000,
        max_sessions: 4,
        pids_limit: 64,
        memory_limit: "256m".to_owned(),
        cpu_limit: "1".to_owned(),
        idle_ttl: Duration::from_secs(60),
        docker_cli: "docker".to_owned(),
        docker_host: None,
        runtime_class: None,
        native_task_root: None,
        native_task_host_root: None,
    }
}

fn spec(root: &std::path::Path) -> GitJobSpec {
    GitJobSpec {
        job_id: Uuid::new_v4(),
        operation_id: Uuid::new_v4(),
        owner: RunnerOwner::ApplicationPrincipal {
            application_id: 42,
            workspace_binding_id: 7,
            principal_digest: "a".repeat(64),
        },
        application_id: 42,
        workspace_binding_id: 7,
        principal_digest: "a".repeat(64),
        code_project_id: Uuid::new_v4(),
        authorization_generation: Uuid::new_v4(),
        canonical_branch: "main".to_owned(),
        expected_base: Some(BASE.to_owned()),
        reserved_bytes: 8 * 1024 * 1024,
        deadline_unix_ms: now_ms() + 60_000,
        workspace_root: root.to_string_lossy().into_owned(),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_millis() as i64
}

fn workspace(root: &std::path::Path) {
    let git = root.join(".git");
    std::fs::create_dir_all(&git).expect("git dir");
    std::fs::write(
        git.join("config"),
        "[core]\n\trepositoryformatversion = 0\n",
    )
    .expect("git config");
}

fn bundle(clone: &GitCloneConfig, spec: &GitJobSpec, head: &str) -> VerifiedGitBundle {
    let workspace =
        CloneWorkspace::create(clone, spec.job_id, spec.operation_id).expect("workspace");
    VerifiedGitBundle::store(
        workspace,
        GitBundleIdentity {
            job_id: spec.job_id,
            operation_id: spec.operation_id,
            application_id: spec.application_id,
            workspace_binding_id: spec.workspace_binding_id,
            principal_digest: spec.principal_digest.clone(),
            code_project_id: spec.code_project_id,
            authorization_generation: spec.authorization_generation,
            branch: spec.canonical_branch.clone(),
            expected_base: spec.expected_base.clone(),
            head: head.to_owned(),
            object_format: GitObjectFormat::Sha1,
        },
        b"bundle-data",
        1024 * 1024,
        super::clone_fetch::TreeStats {
            entries: 1,
            total_bytes: 5,
            files: 1,
            symlinks: 0,
            gitlinks: 0,
        },
        super::clone_fetch::ObjectStats {
            objects: 4,
            total_bytes: 100,
        },
    )
    .expect("bundle")
}

fn runner(config: &RunnerManagerConfig) -> Arc<LocalRunnerService> {
    LocalRunnerService::new(
        DirectRunnerBackend::new(),
        Arc::new(tokio::sync::RwLock::new(config.clone())),
    )
}

fn fixture(
    state: WorkspaceState,
) -> (
    tempfile::TempDir,
    GitJobSpec,
    RunnerManagerConfig,
    GitCloneConfig,
    Arc<FakeExecutor>,
    Arc<LocalRunnerService>,
) {
    let root = private_dir();
    workspace(root.path());
    let spec = spec(root.path());
    let config = config(root.path());
    let clone = clone_config(root.path());
    let executor = Arc::new(FakeExecutor {
        state: Arc::new(Mutex::new(state)),
    });
    let runner = runner(&config);
    (root, spec, config, clone, executor, runner)
}

fn default_state() -> WorkspaceState {
    WorkspaceState {
        head: BASE.to_owned(),
        branch: "main".to_owned(),
        clean: true,
        ancestor: true,
        ..WorkspaceState::default()
    }
}

async fn run(
    spec: &GitJobSpec,
    config: &RunnerManagerConfig,
    clone: &GitCloneConfig,
    executor: &FakeExecutor,
    runner: &LocalRunnerService,
) -> Result<super::update::UpdateSuccess, super::update::UpdateFailure> {
    let bundle = bundle(clone, spec, HEAD);
    let (_tx, cancel) = watch::channel(false);
    update_verified_bundle(spec, config, clone, bundle, executor, runner, cancel).await
}

#[tokio::test]
async fn fast_forward_publishes_descendant_and_writes_completed_receipt() {
    let (root, spec, config, clone, executor, runner) = fixture(default_state());
    let success = run(&spec, &config, &clone, executor.as_ref(), runner.as_ref())
        .await
        .expect("ff success");
    assert_eq!(success.head, HEAD);
    assert_eq!(executor.state.lock().unwrap().head, HEAD);
    let receipt = read_receipt(
        root.path(),
        &super::receipt::GitReceipt::read_request(
            &super::receipt::GitReceipt {
                version: 1,
                job_id: spec.job_id,
                operation_id: spec.operation_id,
                owner: spec.owner.clone(),
                canonical_branch: spec.canonical_branch.clone(),
                base: Some(BASE.to_owned()),
                head: HEAD.to_owned(),
                workspace_root_sha256: super::receipt::workspace_root_sha256(&spec.workspace_root),
                phase: GitReceiptPhase::Completed,
                published: true,
                cleanup_confirmed: true,
            },
            1,
            &spec.workspace_root,
            now_ms() + 60_000,
        ),
    )
    .expect("receipt");
    assert!(receipt.is_confirmed());
    assert!(root.path().join("receipts").exists());
}

#[tokio::test]
async fn dirty_workspace_is_refused_without_effect() {
    let (_root, spec, config, clone, executor, runner) = fixture(WorkspaceState {
        clean: false,
        ..default_state()
    });
    let failure = run(&spec, &config, &clone, executor.as_ref(), runner.as_ref())
        .await
        .expect_err("dirty refused");
    assert_eq!(failure.code, UpdateFailureCode::Conflict);
    assert_eq!(executor.state.lock().unwrap().read_tree_calls, 0);
}

#[tokio::test]
async fn wrong_head_is_refused_without_effect() {
    let (_root, spec, config, clone, executor, runner) = fixture(WorkspaceState {
        head: OTHER.to_owned(),
        ..default_state()
    });
    let failure = run(&spec, &config, &clone, executor.as_ref(), runner.as_ref())
        .await
        .expect_err("wrong head");
    assert_eq!(failure.code, UpdateFailureCode::Conflict);
    assert_eq!(executor.state.lock().unwrap().read_tree_calls, 0);
}

#[tokio::test]
async fn wrong_branch_is_refused_without_effect() {
    let (_root, spec, config, clone, executor, runner) = fixture(WorkspaceState {
        branch: "other".to_owned(),
        ..default_state()
    });
    let failure = run(&spec, &config, &clone, executor.as_ref(), runner.as_ref())
        .await
        .expect_err("wrong branch");
    assert_eq!(failure.code, UpdateFailureCode::Conflict);
}

#[tokio::test]
async fn non_fast_forward_is_refused_without_effect() {
    let (_root, spec, config, clone, executor, runner) = fixture(WorkspaceState {
        ancestor: false,
        ..default_state()
    });
    let failure = run(&spec, &config, &clone, executor.as_ref(), runner.as_ref())
        .await
        .expect_err("non ff");
    assert_eq!(failure.code, UpdateFailureCode::Conflict);
    assert_eq!(executor.state.lock().unwrap().read_tree_calls, 0);
}

#[tokio::test]
async fn missing_expected_base_is_refused() {
    let (_root, mut spec, config, clone, executor, runner) = fixture(default_state());
    spec.expected_base = None;
    let failure = run(&spec, &config, &clone, executor.as_ref(), runner.as_ref())
        .await
        .expect_err("missing base");
    assert_eq!(failure.code, UpdateFailureCode::Unavailable);
}

#[tokio::test]
async fn read_tree_failure_rolls_back_and_keeps_ref() {
    let (_root, spec, config, clone, executor, runner) = fixture(WorkspaceState {
        fail_read_tree: true,
        ..default_state()
    });
    let failure = run(&spec, &config, &clone, executor.as_ref(), runner.as_ref())
        .await
        .expect_err("read tree failed");
    assert_eq!(failure.code, UpdateFailureCode::Conflict);
    assert!(failure.cleanup_confirmed);
    assert_eq!(executor.state.lock().unwrap().head, BASE);
}

#[tokio::test]
async fn cas_failure_rolls_back_cleanly() {
    let (_root, spec, config, clone, executor, runner) = fixture(WorkspaceState {
        fail_cas: true,
        ..default_state()
    });
    let failure = run(&spec, &config, &clone, executor.as_ref(), runner.as_ref())
        .await
        .expect_err("cas failed");
    assert_eq!(failure.code, UpdateFailureCode::Conflict);
    assert!(failure.cleanup_confirmed);
    assert_eq!(executor.state.lock().unwrap().head, BASE);
}

#[tokio::test]
async fn external_head_movement_is_uncertain() {
    let (_root, spec, config, clone, executor, runner) = fixture(WorkspaceState {
        external_move_on_cas: true,
        ..default_state()
    });
    let failure = run(&spec, &config, &clone, executor.as_ref(), runner.as_ref())
        .await
        .expect_err("external movement");
    assert!(!failure.cleanup_confirmed);
}

#[tokio::test]
async fn failed_rollback_is_uncertain_and_retains_verified_receipt() {
    let (root, spec, config, clone, executor, runner) = fixture(WorkspaceState {
        fail_read_tree: true,
        fail_rollback: true,
        ..default_state()
    });
    let failure = run(&spec, &config, &clone, executor.as_ref(), runner.as_ref())
        .await
        .expect_err("rollback failed");
    assert_eq!(failure.code, UpdateFailureCode::ExecutionFailed);
    assert!(!failure.cleanup_confirmed);
    assert!(
        root.path()
            .join("receipts")
            .join(format!("{}.json", spec.job_id))
            .exists()
    );
}

#[tokio::test]
async fn cancel_before_effects_leaves_workspace_unchanged() {
    let (_root, spec, config, clone, executor, runner) = fixture(default_state());
    let bundle = bundle(&clone, &spec, HEAD);
    let (tx, cancel) = watch::channel(true);
    drop(tx);
    let failure = update_verified_bundle(
        &spec,
        &config,
        &clone,
        bundle,
        executor.as_ref(),
        runner.as_ref(),
        cancel,
    )
    .await
    .expect_err("cancelled");
    assert_eq!(failure.code, UpdateFailureCode::Cancelled);
    assert_eq!(executor.state.lock().unwrap().head, BASE);
}

#[tokio::test]
async fn successful_operation_cleanup_never_deletes_receipt() {
    let (root, spec, config, clone, executor, runner) = fixture(default_state());
    run(&spec, &config, &clone, executor.as_ref(), runner.as_ref())
        .await
        .expect("success");
    let path = root
        .path()
        .join("receipts")
        .join(format!("{}.json", spec.job_id));
    let first = std::fs::read(&path).expect("receipt bytes");
    let _ = run(&spec, &config, &clone, executor.as_ref(), runner.as_ref()).await;
    let second = std::fs::read(&path).expect("receipt retained");
    assert_eq!(first, second);
}

#[tokio::test]
async fn preflight_rejects_unsafe_git_config() {
    let (root, spec, config, clone, executor, runner) = fixture(default_state());
    std::fs::write(
        root.path().join(".git/config"),
        "[include]\n\tpath = /tmp/evil\n",
    )
    .expect("config");
    let failure = run(&spec, &config, &clone, executor.as_ref(), runner.as_ref())
        .await
        .expect_err("unsafe config");
    assert_eq!(failure.code, UpdateFailureCode::Conflict);
}

#[tokio::test]
async fn preflight_rejects_root_outside_manager_workspace() {
    let (_root, mut spec, config, clone, executor, runner) = fixture(default_state());
    spec.workspace_root = "/tmp/not-managed".to_owned();
    let failure = run(&spec, &config, &clone, executor.as_ref(), runner.as_ref())
        .await
        .expect_err("outside root");
    assert_eq!(failure.code, UpdateFailureCode::Conflict);
}

#[tokio::test]
async fn branch_and_head_are_published_together() {
    let (_root, spec, config, clone, executor, runner) = fixture(default_state());
    run(&spec, &config, &clone, executor.as_ref(), runner.as_ref())
        .await
        .expect("success");
    let state = executor.state.lock().unwrap();
    assert_eq!(state.head, HEAD);
    assert_eq!(state.branch, "main");
    let _ = PathBuf::new();
}
