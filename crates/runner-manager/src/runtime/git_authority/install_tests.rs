use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use runner_protocol::{GitJobSpec, GitObjectFormat, RunnerOwner};
use tokio::sync::watch;
use uuid::Uuid;

use super::clone_bundle::{GitBundleIdentity, VerifiedGitBundle};
use super::clone_plan::{
    GitCloneCommand, GitCloneConfig, GitCloneExecutor, GitCloneLimits, GitPlan, GitPlanFuture,
    GitPlanOutcome,
};
use super::clone_volume::GitVolumePlan;
use super::clone_workspace::CloneWorkspace;
use super::install::{InstallFailureCode, install_verified_bundle};
use crate::config::{RunnerBackendKind, RunnerManagerConfig};
use crate::runtime::direct::DirectRunnerBackend;
use crate::runtime::local_service::LocalRunnerService;

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct FakeExecutor {
    export: Vec<u8>,
    export_failure: bool,
    remove_failure: bool,
    install_head: String,
    removed: Arc<Mutex<Vec<String>>>,
}

impl FakeExecutor {
    fn new() -> Self {
        Self {
            export: tar_bytes(),
            export_failure: false,
            remove_failure: false,
            install_head: HEAD.to_owned(),
            removed: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn removed(&self) -> Vec<String> {
        self.removed.lock().expect("removed").clone()
    }
}

impl GitCloneExecutor for FakeExecutor {
    fn run_plan<'a>(
        &'a self,
        plan: &'a GitPlan,
        _cancel: Option<watch::Receiver<bool>>,
    ) -> GitPlanFuture<'a> {
        Box::pin(async move {
            let outcome = match &plan.command {
                GitCloneCommand::InstallShowObjectFormat => outcome(0, b"sha1\n"),
                GitCloneCommand::InstallVerifyHead { .. }
                | GitCloneCommand::InstallReadBranchRef { .. } => {
                    outcome(0, format!("{}\n", self.install_head).as_bytes())
                }
                GitCloneCommand::InstallExport => {
                    if self.export_failure {
                        return Err(super::clone_plan::GitCloneExecutorError::Failed);
                    }
                    outcome(0, &self.export)
                }
                _ => outcome(0, b""),
            };
            Ok(outcome)
        })
    }

    fn create_volume<'a>(&'a self, plan: &'a GitVolumePlan) -> GitPlanFuture<'a> {
        Box::pin(async move { Ok(outcome(0, format!("{}\n", plan.name).as_bytes())) })
    }

    fn remove_volume<'a>(&'a self, plan: &'a GitVolumePlan) -> GitPlanFuture<'a> {
        self.removed
            .lock()
            .expect("removed")
            .push(plan.name.clone());
        let fail = self.remove_failure;
        Box::pin(async move {
            if fail {
                return Ok(GitPlanOutcome {
                    exit_code: 1,
                    stdout: Vec::new(),
                    stderr: b"daemon unavailable\n".to_vec(),
                });
            }
            Ok(outcome(0, b""))
        })
    }
}

fn outcome(exit_code: i32, stdout: &[u8]) -> GitPlanOutcome {
    GitPlanOutcome {
        exit_code,
        stdout: stdout.to_vec(),
        stderr: Vec::new(),
    }
}

fn regular(builder: &mut tar::Builder<Vec<u8>>, path: &str, body: &[u8], mode: u32) {
    let mut header = tar::Header::new_gnu();
    header.set_size(body.len() as u64);
    header.set_mode(mode);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    builder
        .append_data(&mut header, path, body)
        .expect("append");
}

fn tar_bytes() -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    regular(&mut builder, "./file.txt", b"hello", 0o644);
    regular(
        &mut builder,
        "./.git/HEAD",
        b"ref: refs/heads/main\n",
        0o644,
    );
    regular(
        &mut builder,
        "./.git/config",
        b"[core]\n\trepositoryformatversion = 0\n",
        0o644,
    );
    regular(
        &mut builder,
        "./.git/refs/heads/main",
        format!("{HEAD}\n").as_bytes(),
        0o644,
    );
    builder.into_inner().expect("tar")
}

fn private_dir() -> tempfile::TempDir {
    let base = std::env::var_os("HOME").expect("home directory");
    let temp = tempfile::Builder::new().tempdir_in(&base).expect("temp");
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).expect("mode");
    temp
}

fn uid_gid() -> (u32, u32) {
    (
        rustix::process::geteuid().as_raw(),
        rustix::process::getegid().as_raw(),
    )
}

fn clone_config(root: &std::path::Path) -> GitCloneConfig {
    let (uid, gid) = uid_gid();
    GitCloneConfig {
        private_root: root.to_path_buf(),
        host_private_root: root.to_path_buf(),
        limits: GitCloneLimits {
            image: format!("helper@sha256:{}", "a".repeat(64)),
            network: "df-git-net".to_owned(),
            broker_host: "broker.example.invalid".to_owned(),
            uid,
            gid,
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

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn spec(workspace_root: &std::path::Path, project: Uuid) -> GitJobSpec {
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
        code_project_id: project,
        authorization_generation: Uuid::new_v4(),
        canonical_branch: "main".to_owned(),
        expected_base: None,
        reserved_bytes: 8 * 1024 * 1024,
        deadline_unix_ms: now_ms() + 60_000,
        workspace_root: workspace_root.to_string_lossy().into_owned(),
    }
}

fn workspace_with_target(project: Uuid) -> (tempfile::TempDir, std::path::PathBuf) {
    let root = private_dir();
    let resources = root.path().join("apps").join("42").join("resources");
    std::fs::create_dir_all(&resources).expect("resources");
    for path in [
        root.path().join("apps"),
        root.path().join("apps").join("42"),
        resources.clone(),
    ] {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).expect("mode");
    }
    let target = resources.join(desk_foreman::workspace::resource_workspace_hash(
        "code_project",
        &project.to_string(),
    ));
    (root, target)
}

fn bundle(clone: &GitCloneConfig, spec: &GitJobSpec) -> VerifiedGitBundle {
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
            expected_base: None,
            head: HEAD.to_owned(),
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

#[tokio::test]
async fn installs_and_publishes_only_after_cleanup() {
    let project = Uuid::new_v4();
    let (workspace, target) = workspace_with_target(project);
    let config = config(workspace.path());
    let clone = clone_config(workspace.path());
    let request = spec(&target, project);
    let executor = FakeExecutor::new();
    let runner = runner(&config);
    let (_tx, cancel) = watch::channel(false);
    let success = install_verified_bundle(
        &request,
        &config,
        &clone,
        bundle(&clone, &request),
        &executor,
        runner.as_ref(),
        cancel,
    )
    .await
    .expect("install");
    assert_eq!(success.head, HEAD);
    assert!(target.join(".git/HEAD").is_file());
    assert_eq!(executor.removed().len(), 1);
}

#[tokio::test]
async fn wrong_target_is_refused_before_volume_effects() {
    let project = Uuid::new_v4();
    let (workspace, _target) = workspace_with_target(project);
    let config = config(workspace.path());
    let clone = clone_config(workspace.path());
    let mut request = spec(&workspace.path().join("elsewhere"), project);
    request.workspace_root = workspace
        .path()
        .join("elsewhere")
        .to_string_lossy()
        .into_owned();
    let executor = FakeExecutor::new();
    let runner = runner(&config);
    let (_tx, cancel) = watch::channel(false);
    let failure = install_verified_bundle(
        &request,
        &config,
        &clone,
        bundle(&clone, &request),
        &executor,
        runner.as_ref(),
        cancel,
    )
    .await
    .expect_err("wrong target");
    assert_eq!(failure.code, InstallFailureCode::Unavailable);
    assert!(failure.cleanup_confirmed);
    assert!(executor.removed().is_empty());
}

#[tokio::test]
async fn malformed_export_is_a_clean_failure_after_volume_removal() {
    let project = Uuid::new_v4();
    let (workspace, target) = workspace_with_target(project);
    let config = config(workspace.path());
    let clone = clone_config(workspace.path());
    let request = spec(&target, project);
    let mut executor = FakeExecutor::new();
    executor.export = b"not-a-tar".to_vec();
    let runner = runner(&config);
    let (_tx, cancel) = watch::channel(false);
    let failure = install_verified_bundle(
        &request,
        &config,
        &clone,
        bundle(&clone, &request),
        &executor,
        runner.as_ref(),
        cancel,
    )
    .await
    .expect_err("archive");
    assert_eq!(failure.code, InstallFailureCode::ExecutionFailed);
    assert!(failure.cleanup_confirmed);
    assert_eq!(executor.removed().len(), 1);
    assert!(!target.exists());
}

#[tokio::test]
async fn uncertain_helper_retains_resources() {
    let project = Uuid::new_v4();
    let (workspace, target) = workspace_with_target(project);
    let config = config(workspace.path());
    let clone = clone_config(workspace.path());
    let request = spec(&target, project);
    let mut executor = FakeExecutor::new();
    executor.export_failure = true;
    let runner = runner(&config);
    let (_tx, cancel) = watch::channel(false);
    let failure = install_verified_bundle(
        &request,
        &config,
        &clone,
        bundle(&clone, &request),
        &executor,
        runner.as_ref(),
        cancel,
    )
    .await
    .expect_err("uncertain");
    assert_eq!(failure.code, InstallFailureCode::ExecutionFailed);
    assert!(!failure.cleanup_confirmed);
    assert!(executor.removed().is_empty());
    assert!(!target.exists());
}

#[tokio::test]
async fn cancellation_is_clean_before_any_helper() {
    let project = Uuid::new_v4();
    let (workspace, target) = workspace_with_target(project);
    let config = config(workspace.path());
    let clone = clone_config(workspace.path());
    let request = spec(&target, project);
    let executor = FakeExecutor::new();
    let runner = runner(&config);
    let (tx, cancel) = watch::channel(false);
    tx.send(true).expect("cancel");
    let failure = install_verified_bundle(
        &request,
        &config,
        &clone,
        bundle(&clone, &request),
        &executor,
        runner.as_ref(),
        cancel,
    )
    .await
    .expect_err("cancelled");
    assert_eq!(failure.code, InstallFailureCode::Cancelled);
    assert!(failure.cleanup_confirmed);
    assert!(executor.removed().is_empty());
}

#[allow(dead_code)]
fn _os(_: &OsStr) {}

fn git_command() -> std::process::Command {
    let mut command = std::process::Command::new("git");
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com");
    command
}

fn real_repo_tar() -> (Vec<u8>, String) {
    let temp = private_dir();
    let repo = temp.path().join("repo");
    assert!(
        git_command()
            .args(["init", "-b", "main"])
            .arg(&repo)
            .status()
            .expect("init")
            .success()
    );
    std::fs::write(repo.join("README.md"), b"hello\n").expect("file");
    assert!(
        git_command()
            .args(["-C", repo.to_str().expect("repo"), "add", "."])
            .status()
            .expect("add")
            .success()
    );
    assert!(
        git_command()
            .args(["-C", repo.to_str().expect("repo"), "commit", "-m", "one"])
            .status()
            .expect("commit")
            .success()
    );
    let head = String::from_utf8(
        git_command()
            .args(["-C", repo.to_str().expect("repo"), "rev-parse", "HEAD"])
            .output()
            .expect("head")
            .stdout,
    )
    .expect("utf8")
    .trim()
    .to_owned();
    normalize_modes(&repo);
    let mut builder = tar::Builder::new(Vec::new());
    builder.append_dir_all(".", &repo).expect("append repo");
    (builder.into_inner().expect("tar"), head)
}

#[tokio::test]
async fn benign_real_git_checkout_is_usable_after_install() {
    let (export, head) = real_repo_tar();
    let project = Uuid::new_v4();
    let (workspace, target) = workspace_with_target(project);
    let config = config(workspace.path());
    let clone = clone_config(workspace.path());
    let mut request = spec(&target, project);
    request.reserved_bytes = 16 * 1024 * 1024;
    let mut executor = FakeExecutor::new();
    executor.export = export;
    executor.install_head = head.clone();
    let runner = runner(&config);
    let (_tx, cancel) = watch::channel(false);
    let workspace_bundle = bundle_with_head(&clone, &request, &head);
    let success = install_verified_bundle(
        &request,
        &config,
        &clone,
        workspace_bundle,
        &executor,
        runner.as_ref(),
        cancel,
    )
    .await
    .expect("install");
    assert_eq!(success.head, head);
    let status = git_command()
        .args([
            "-C",
            target.to_str().expect("target"),
            "status",
            "--porcelain",
        ])
        .output()
        .expect("status");
    assert!(status.status.success());
    assert!(status.stdout.is_empty());
    let resolved = git_command()
        .args(["-C", target.to_str().expect("target"), "rev-parse", "HEAD"])
        .output()
        .expect("rev-parse");
    assert_eq!(String::from_utf8_lossy(&resolved.stdout).trim(), head);
    let log = git_command()
        .args(["-C", target.to_str().expect("target"), "log", "--format=%H"])
        .output()
        .expect("log");
    assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), head);
}

fn normalize_modes(path: &std::path::Path) {
    use std::os::unix::fs::FileTypeExt;
    for entry in std::fs::read_dir(path).expect("read_dir") {
        let entry = entry.expect("entry");
        let file_type = entry.file_type().expect("type");
        if file_type.is_symlink() || file_type.is_fifo() || file_type.is_socket() {
            continue;
        }
        let mode = if file_type.is_dir() { 0o755 } else { 0o644 };
        std::fs::set_permissions(entry.path(), std::fs::Permissions::from_mode(mode))
            .expect("mode");
        if file_type.is_dir() {
            normalize_modes(&entry.path());
        }
    }
}

fn bundle_with_head(clone: &GitCloneConfig, spec: &GitJobSpec, head: &str) -> VerifiedGitBundle {
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
            expected_base: None,
            head: head.to_owned(),
            object_format: GitObjectFormat::Sha1,
        },
        b"bundle-data",
        1024 * 1024,
        super::clone_fetch::TreeStats {
            entries: 1,
            total_bytes: 6,
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

#[tokio::test]
async fn insufficient_install_reservation_is_resource_limit_before_resources() {
    let project = Uuid::new_v4();
    let (workspace, target) = workspace_with_target(project);
    let config = config(workspace.path());
    let clone = clone_config(workspace.path());
    let mut request = spec(&target, project);
    request.reserved_bytes = 1;
    let executor = FakeExecutor::new();
    let runner = runner(&config);
    let (_tx, cancel) = watch::channel(false);
    let failure = install_verified_bundle(
        &request,
        &config,
        &clone,
        bundle(&clone, &request),
        &executor,
        runner.as_ref(),
        cancel,
    )
    .await
    .expect_err("reservation");
    assert_eq!(failure.code, InstallFailureCode::ResourceLimit);
    assert!(failure.cleanup_confirmed);
    assert!(executor.removed().is_empty());
    assert!(!target.exists());
}

#[tokio::test]
async fn post_publication_cleanup_failure_preserves_target_and_reports_unconfirmed() {
    let project = Uuid::new_v4();
    let (workspace, target) = workspace_with_target(project);
    let config = config(workspace.path());
    let clone = clone_config(workspace.path());
    let mut request = spec(&target, project);
    request.reserved_bytes = 16 * 1024 * 1024;
    let mut executor = FakeExecutor::new();
    executor.remove_failure = true;
    let runner = runner(&config);
    let (_tx, cancel) = watch::channel(false);
    let failure = install_verified_bundle(
        &request,
        &config,
        &clone,
        bundle(&clone, &request),
        &executor,
        runner.as_ref(),
        cancel,
    )
    .await
    .expect_err("cleanup");
    assert_eq!(failure.code, InstallFailureCode::ExecutionFailed);
    assert!(!failure.cleanup_confirmed);
    assert!(failure.retained_volume.is_some());
    assert!(target.join(".git/HEAD").is_file());
}
