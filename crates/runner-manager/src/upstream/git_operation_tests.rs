use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use runner_protocol::{GitJobCredential, GitJobSpec, GitObjectFormat, RunnerOwner};
use tokio::sync::watch;
use uuid::Uuid;

use super::git_operation::{
    GitCloneFailureCode, GitCredentialFuture, GitCredentialSource, prepare_git_clone,
};
use crate::config::{RunnerBackendKind, RunnerManagerConfig};
use crate::runtime::git_authority::clone_plan::{
    GitCloneCommand, GitCloneExecutor, GitPlan, GitPlanFuture, GitPlanOutcome,
};
use crate::runtime::git_authority::clone_volume::GitVolumePlan;
use crate::runtime::git_authority::clone_workspace::MOUNT_PROOF_BLOB_SHA1;

#[derive(Clone)]
struct FakeCredential {
    credential: GitJobCredential,
}

impl GitCredentialSource for FakeCredential {
    fn fetch<'a>(
        &'a self,
        _spec: &'a GitJobSpec,
        _deadline: tokio::time::Instant,
    ) -> GitCredentialFuture<'a> {
        Box::pin(async move { Ok(self.credential.clone()) })
    }
}

struct FakeExecutor {
    calls: Arc<Mutex<Vec<String>>>,
    proof: Vec<u8>,
    fail_fetch: bool,
    local_ref: Vec<u8>,
    create: Option<GitPlanOutcome>,
    remove: Option<GitPlanOutcome>,
    remove_missing: bool,
    credential_dir: Option<std::path::PathBuf>,
    credential_observations: Arc<Mutex<Vec<(String, bool)>>>,
    install_export: Vec<u8>,
}

impl FakeExecutor {
    fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            proof: format!("{MOUNT_PROOF_BLOB_SHA1}\n").into_bytes(),
            fail_fetch: false,
            local_ref: format!("{}\n", "a".repeat(40)).into_bytes(),
            create: None,
            remove: None,
            remove_missing: false,
            credential_dir: None,
            credential_observations: Arc::new(Mutex::new(Vec::new())),
            install_export: Vec::new(),
        }
    }

    fn with_credential_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.credential_dir = Some(dir);
        self
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("calls").clone()
    }

    fn credential_observations(&self) -> Vec<(String, bool)> {
        self.credential_observations
            .lock()
            .expect("observations")
            .clone()
    }
}

impl GitCloneExecutor for FakeExecutor {
    fn run_plan<'a>(
        &'a self,
        plan: &'a GitPlan,
        _cancel: Option<watch::Receiver<bool>>,
    ) -> GitPlanFuture<'a> {
        let command = format!("{:?}", plan.command);
        self.calls.lock().expect("calls").push(command.clone());
        if let Some(dir) = &self.credential_dir {
            self.credential_observations
                .lock()
                .expect("observations")
                .push((command.clone(), !dir.join("password").exists()));
        }
        Box::pin(async move {
            let outcome = match &plan.command {
                GitCloneCommand::Proof { .. } => outcome(0, &self.proof),
                GitCloneCommand::NetLsRemote { branch, .. } => outcome(
                    0,
                    format!("{}\trefs/heads/{branch}\n", "a".repeat(40)).as_bytes(),
                ),
                GitCloneCommand::FetchInit { .. } => outcome(0, b""),
                GitCloneCommand::Fetch { .. } if self.fail_fetch => outcome(1, b""),
                GitCloneCommand::Fetch { .. } => outcome(0, b""),
                GitCloneCommand::Verify { head, .. } => outcome(0, format!("{head}\n").as_bytes()),
                GitCloneCommand::ShowObjectFormat => outcome(0, b"sha1\n"),
                GitCloneCommand::Fsck => outcome(0, b""),
                GitCloneCommand::ListTreeSizes { .. } => outcome(
                    0,
                    format!("100644 blob {} 5\tfile\0", "b".repeat(40)).as_bytes(),
                ),
                GitCloneCommand::ListObjectSizes => {
                    outcome(0, format!("{} blob 5\n", "b".repeat(40)).as_bytes())
                }
                GitCloneCommand::MergeBaseIsAncestor { .. } => outcome(0, b""),
                GitCloneCommand::ReadLocalRef { .. } => outcome(0, &self.local_ref),
                GitCloneCommand::CatFileCommit { .. } => outcome(0, b""),
                GitCloneCommand::BundleCreate { .. } => outcome(0, b"bundle-data"),
                GitCloneCommand::InstallShowObjectFormat => outcome(0, b"sha1\n"),
                GitCloneCommand::InstallVerifyHead { .. }
                | GitCloneCommand::InstallReadBranchRef { .. } => {
                    outcome(0, format!("{}\n", "a".repeat(40)).as_bytes())
                }
                GitCloneCommand::InstallExport => outcome(0, &self.install_export),
                GitCloneCommand::InstallClone { .. }
                | GitCloneCommand::InstallRemoveOrigin
                | GitCloneCommand::InstallCheckout { .. }
                | GitCloneCommand::InstallFsck
                | GitCloneCommand::InstallStatusClean => outcome(0, b""),
                _ => outcome(0, b""),
            };
            Ok(outcome)
        })
    }

    fn create_volume<'a>(&'a self, plan: &'a GitVolumePlan) -> GitPlanFuture<'a> {
        self.calls
            .lock()
            .expect("calls")
            .push(format!("volume-create:{}", plan.name));
        let configured = self.create.clone();
        Box::pin(async move {
            Ok(configured.unwrap_or_else(|| outcome(0, format!("{}\n", plan.name).as_bytes())))
        })
    }

    fn remove_volume<'a>(&'a self, plan: &'a GitVolumePlan) -> GitPlanFuture<'a> {
        self.calls
            .lock()
            .expect("calls")
            .push(format!("volume-remove:{}", plan.name));
        let configured = self.remove.clone();
        let remove_missing = self.remove_missing;
        Box::pin(async move {
            if remove_missing {
                return Ok(GitPlanOutcome {
                    exit_code: 1,
                    stdout: Vec::new(),
                    stderr: format!("no such volume: {}\n", plan.name).into_bytes(),
                });
            }
            Ok(configured.unwrap_or_else(|| outcome(0, b"")))
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

fn private_root() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let home = std::env::var_os("HOME").expect("HOME");
    let base = std::path::PathBuf::from(home).join(".cache/ai/1180-dfi3a1c1");
    std::fs::create_dir_all(&base).expect("base");
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).expect("base mode");
    let temp = tempfile::Builder::new()
        .prefix("git-operation-")
        .tempdir_in(&base)
        .expect("private root");
    std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).expect("mode");
    temp
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
        git_max_tree_entries: Some(1024),
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

fn spec(code_project_id: Uuid, deadline_unix_ms: i64) -> GitJobSpec {
    GitJobSpec {
        job_id: Uuid::new_v4(),
        operation_id: Uuid::new_v4(),
        owner: RunnerOwner::ApplicationPrincipal {
            application_id: 1,
            workspace_binding_id: 2,
            principal_digest: "a".repeat(64),
        },
        application_id: 1,
        workspace_binding_id: 2,
        principal_digest: "a".repeat(64),
        code_project_id,
        authorization_generation: Uuid::new_v4(),
        canonical_branch: "main".to_owned(),
        expected_base: None,
        reserved_bytes: 3 * 1024 * 1024,
        deadline_unix_ms,
        workspace_root: "/private/workspace".to_owned(),
    }
}

fn credential(code_project_id: Uuid, deadline_unix_ms: i64) -> GitJobCredential {
    GitJobCredential {
        username: "broker".to_owned(),
        password: format!("smcpw1.{}.{}", Uuid::new_v4(), "a".repeat(64)),
        remote_url: format!(
            "https://broker.example.invalid/api/internal/code-projects/{code_project_id}/git"
        ),
        deadline_unix_ms,
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[tokio::test]
async fn git_operation_prepares_one_bounded_bundle_without_a_result() {
    let root = private_root();
    let code_project_id = Uuid::new_v4();
    let deadline = now_ms() + 60_000;
    let request = spec(code_project_id, deadline);
    let source = FakeCredential {
        credential: credential(code_project_id, deadline),
    };
    let executor = FakeExecutor::new();
    let (_tx, rx) = watch::channel(false);
    let bundle = prepare_git_clone(&request, &config(root.path()), &source, &executor, rx)
        .await
        .expect("prepared bundle");
    assert_eq!(bundle.identity().head, "a".repeat(40));
    assert_eq!(bundle.identity().object_format, GitObjectFormat::Sha1);
    assert_eq!(bundle.bundle_bytes(), 11);
    let calls = executor.calls();
    assert!(calls.iter().any(|call| call.contains("Proof")));
    assert!(calls.iter().any(|call| call.starts_with("volume-create:")));
    assert!(calls.iter().any(|call| call.contains("NetLsRemote")));
    assert!(calls.iter().any(|call| call.contains("FetchInit")));
    assert!(calls.iter().any(|call| call.contains("BundleCreate")));
    assert!(calls.iter().any(|call| call.starts_with("volume-remove:")));
    bundle.cleanup().expect("cleanup");
}

#[tokio::test]
async fn git_operation_proof_failure_stops_before_volume_or_secret_effects() {
    let root = private_root();
    let code_project_id = Uuid::new_v4();
    let deadline = now_ms() + 60_000;
    let request = spec(code_project_id, deadline);
    let source = FakeCredential {
        credential: credential(code_project_id, deadline),
    };
    let mut executor = FakeExecutor::new();
    executor.proof = b"wrong\n".to_vec();
    let (_tx, rx) = watch::channel(false);
    let failure = prepare_git_clone(&request, &config(root.path()), &source, &executor, rx)
        .await
        .expect_err("proof failure");
    assert_eq!(failure.code, GitCloneFailureCode::ProofFailed);
    assert!(!failure.retained);
    let calls = executor.calls();
    assert!(calls.iter().all(|call| !call.starts_with("volume-create:")));
    assert!(calls.iter().all(|call| !call.contains("NetLsRemote")));
    assert!(std::fs::read_dir(root.path()).expect("root").all(|entry| {
        !entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .starts_with("dfgit-op-")
    }));
}

#[tokio::test]
async fn git_operation_rejects_under_reserved_and_pretrue_cancel_before_effects() {
    let root = private_root();
    let code_project_id = Uuid::new_v4();
    let deadline = now_ms() + 60_000;
    let mut request = spec(code_project_id, deadline);
    request.reserved_bytes = 1024;
    let source = FakeCredential {
        credential: credential(code_project_id, deadline),
    };
    let executor = FakeExecutor::new();
    let (_tx, rx) = watch::channel(false);
    let failure = prepare_git_clone(&request, &config(root.path()), &source, &executor, rx)
        .await
        .expect_err("reservation");
    assert_eq!(failure.code, GitCloneFailureCode::ResourceLimit);
    assert!(executor.calls().is_empty());

    let request = spec(code_project_id, deadline);
    let (_tx, rx) = watch::channel(true);
    let failure = prepare_git_clone(&request, &config(root.path()), &source, &executor, rx)
        .await
        .expect_err("cancelled");
    assert_eq!(failure.code, GitCloneFailureCode::Cancelled);
    assert!(executor.calls().is_empty());
}

#[tokio::test]
async fn credentials_are_absent_before_every_verification_step() {
    let root = private_root();
    let code_project_id = Uuid::new_v4();
    let deadline = now_ms() + 60_000;
    let request = spec(code_project_id, deadline);
    let operation_dir = root.path().join(format!(
        "dfgit-op-{}-{}",
        request.job_id.simple(),
        &request.operation_id.simple().to_string()[..12]
    ));
    let source = FakeCredential {
        credential: credential(code_project_id, deadline),
    };
    let executor = FakeExecutor::new().with_credential_dir(operation_dir.join("credential"));
    let (_tx, rx) = watch::channel(false);
    let bundle = prepare_git_clone(&request, &config(root.path()), &source, &executor, rx)
        .await
        .expect("prepared");
    let observations = executor.credential_observations();
    let fetch_index = observations
        .iter()
        .position(|(call, _)| call.contains("Fetch {"))
        .expect("fetch call");
    for (call, missing) in &observations {
        if call.contains("NetLsRemote") || call.contains("Fetch {") {
            assert!(!missing, "credential missing during {call}");
        }
    }
    for (call, missing) in observations.iter().skip(fetch_index + 1) {
        assert!(missing, "credential still present before {call}");
    }
    bundle.cleanup().expect("cleanup");
}

#[tokio::test]
async fn changed_local_ref_is_a_clean_conflict() {
    let root = private_root();
    let code_project_id = Uuid::new_v4();
    let deadline = now_ms() + 60_000;
    let request = spec(code_project_id, deadline);
    let source = FakeCredential {
        credential: credential(code_project_id, deadline),
    };
    let mut executor = FakeExecutor::new();
    executor.local_ref = format!("{}\n", "b".repeat(40)).into_bytes();
    let (_tx, rx) = watch::channel(false);
    let failure = prepare_git_clone(&request, &config(root.path()), &source, &executor, rx)
        .await
        .expect_err("conflict");
    assert_eq!(failure.code, GitCloneFailureCode::Conflict);
    assert!(!failure.retained);
    assert!(
        !executor
            .calls()
            .iter()
            .any(|call| call.contains("BundleCreate"))
    );
}

#[tokio::test]
async fn unconfirmed_volume_removal_retains_owned_resources() {
    let root = private_root();
    let code_project_id = Uuid::new_v4();
    let deadline = now_ms() + 60_000;
    let request = spec(code_project_id, deadline);
    let source = FakeCredential {
        credential: credential(code_project_id, deadline),
    };
    let mut executor = FakeExecutor::new();
    executor.create = Some(outcome(1, b""));
    executor.remove = Some(GitPlanOutcome {
        exit_code: 1,
        stdout: Vec::new(),
        stderr: b"daemon unavailable\n".to_vec(),
    });
    let (_tx, rx) = watch::channel(false);
    let failure = prepare_git_clone(&request, &config(root.path()), &source, &executor, rx)
        .await
        .expect_err("retained");
    assert_eq!(failure.code, GitCloneFailureCode::VolumeFailed);
    assert!(failure.retained);
    let resources = failure.retained_resources().expect("resources");
    assert!(resources.volume_name.is_some());
    assert!(resources.operation_dir.exists());
    assert!(format!("{failure:?}").contains("retained: true"));
}

#[tokio::test]
async fn missing_volume_after_create_failure_is_not_retained() {
    let root = private_root();
    let code_project_id = Uuid::new_v4();
    let deadline = now_ms() + 60_000;
    let request = spec(code_project_id, deadline);
    let source = FakeCredential {
        credential: credential(code_project_id, deadline),
    };
    let mut executor = FakeExecutor::new();
    executor.create = Some(outcome(1, b""));
    executor.remove_missing = true;
    let (_tx, rx) = watch::channel(false);
    let failure = prepare_git_clone(&request, &config(root.path()), &source, &executor, rx)
        .await
        .expect_err("volume failure");
    assert_eq!(failure.code, GitCloneFailureCode::VolumeFailed);
    assert!(!failure.retained);
    assert!(failure.retained_resources().is_none());
    assert!(std::fs::read_dir(root.path()).expect("root").all(|entry| {
        !entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .starts_with("dfgit-op-")
    }));
}

#[tokio::test]
async fn missing_clone_config_and_past_deadline_fail_before_effects() {
    let root = private_root();
    let code_project_id = Uuid::new_v4();
    let deadline = now_ms() + 60_000;
    let request = spec(code_project_id, deadline);
    let source = FakeCredential {
        credential: credential(code_project_id, deadline),
    };
    let executor = FakeExecutor::new();
    for mutate in [
        |config: &mut RunnerManagerConfig| config.git_network = None,
        |config: &mut RunnerManagerConfig| config.git_broker_host = None,
        |config: &mut RunnerManagerConfig| config.git_max_fetch_volume_bytes = None,
        |config: &mut RunnerManagerConfig| config.git_max_bundle_bytes = Some(64 * 1024 * 1024 + 1),
        |config: &mut RunnerManagerConfig| config.git_max_tree_entries = Some(0),
    ] {
        let mut config = config(root.path());
        mutate(&mut config);
        let (_tx, rx) = watch::channel(false);
        let failure = prepare_git_clone(&request, &config, &source, &executor, rx)
            .await
            .expect_err("invalid config");
        assert_eq!(failure.code, GitCloneFailureCode::InvalidConfig);
        assert!(!failure.retained);
    }
    assert!(executor.calls().is_empty());

    let mut past = spec(code_project_id, now_ms() - 1);
    past.deadline_unix_ms = now_ms() - 1;
    let (_tx, rx) = watch::channel(false);
    let failure = prepare_git_clone(&past, &config(root.path()), &source, &executor, rx)
        .await
        .expect_err("expired");
    assert_eq!(failure.code, GitCloneFailureCode::Timeout);
    assert!(executor.calls().is_empty());
}

#[tokio::test]
async fn git_operation_dispatch_rejects_malformed_or_mismatched_payloads() {
    use crate::runtime::DirectRunnerBackend;
    use crate::runtime::LocalRunnerService;
    use runner_protocol::{RunnerJob, RunnerOwner};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let root = private_root();
    let config = Arc::new(RwLock::new(config(root.path())));
    let runner = LocalRunnerService::new(DirectRunnerBackend::new(), Arc::clone(&config));
    let job = RunnerJob {
        job_id: "gitjob:not-a-uuid".to_owned(),
        kind: "git_operation".to_owned(),
        payload: serde_json::json!({"unexpected": true}),
    };
    let result = super::git_operation::execute_job(runner.as_ref(), &config, job).await;
    assert!(!result.ok);
    assert!(result.result.is_none());
    assert_eq!(result.error.as_deref(), Some("invalid git job payload"));

    let project = Uuid::new_v4();
    let deadline = now_ms() + 60_000;
    let mut request = spec(project, deadline);
    request.owner = RunnerOwner::ApplicationPrincipal {
        application_id: 1,
        workspace_binding_id: 2,
        principal_digest: "a".repeat(64),
    };
    let payload = serde_json::to_value(&request).expect("payload");
    let job = RunnerJob {
        job_id: format!("gitjob:{}", Uuid::new_v4()),
        kind: "git_operation".to_owned(),
        payload,
    };
    let result = super::git_operation::execute_job(runner.as_ref(), &config, job).await;
    assert!(!result.ok);
    assert!(result.result.is_none());
    assert_eq!(result.error.as_deref(), Some("invalid git job payload"));
}

#[tokio::test]
async fn git_operation_dispatch_reports_a_typed_success_after_install_cleanup() {
    use crate::runtime::DirectRunnerBackend;
    use crate::runtime::LocalRunnerService;
    use runner_protocol::{GitJobOutcome, RunnerOwner};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    let private = private_root();
    let workspace = private_root();
    let project = Uuid::new_v4();
    let target = workspace
        .path()
        .join("apps")
        .join("1")
        .join("resources")
        .join(desk_foreman::workspace::resource_workspace_hash(
            "code_project",
            &project.to_string(),
        ));
    std::fs::create_dir_all(target.parent().expect("parent")).expect("target parent");
    for path in [
        workspace.path().join("apps"),
        workspace.path().join("apps").join("1"),
        workspace.path().join("apps").join("1").join("resources"),
    ] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).expect("mode");
    }
    let mut config = config(private.path());
    config.workspace_root = workspace.path().to_path_buf();
    config.host_workspace_root = workspace.path().to_path_buf();
    let deadline = now_ms() + 60_000;
    let mut request = spec(project, deadline);
    request.owner = RunnerOwner::ApplicationPrincipal {
        application_id: 1,
        workspace_binding_id: 2,
        principal_digest: "a".repeat(64),
    };
    request.application_id = 1;
    request.workspace_binding_id = 2;
    request.reserved_bytes = 16 * 1024 * 1024;
    request.workspace_root = target.to_string_lossy().into_owned();
    let source = FakeCredential {
        credential: credential(project, deadline),
    };
    let mut executor = FakeExecutor::new();
    executor.install_export = install_tar();
    let shared = Arc::new(RwLock::new(config.clone()));
    let runner = LocalRunnerService::new(DirectRunnerBackend::new(), shared);
    let (_tx, cancel) = watch::channel(false);
    let job = runner_protocol::RunnerJob {
        job_id: format!("gitjob:{}", request.job_id),
        kind: "git_operation".to_owned(),
        payload: serde_json::to_value(&request).expect("payload"),
    };
    let result = super::git_operation::execute_job_with(
        runner.as_ref(),
        &config,
        job,
        &source,
        &executor,
        cancel,
    )
    .await;
    assert!(result.ok, "unexpected result: {:?}", result.result);
    assert!(result.error.is_none());
    let outcome: GitJobOutcome =
        serde_json::from_value(result.result.expect("result")).expect("outcome");
    match outcome {
        GitJobOutcome::SyncSucceeded {
            operation_id,
            head,
            base,
            published,
            cleanup_confirmed,
        } => {
            assert_eq!(operation_id, request.operation_id);
            assert_eq!(head, "a".repeat(40));
            assert_eq!(base, request.expected_base);
            assert!(published);
            assert!(cleanup_confirmed);
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
    assert!(target.join(".git/HEAD").is_file());
}

fn install_tar() -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    let mut append = |path: &str, body: &[u8]| {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder
            .append_data(&mut header, path, body)
            .expect("append");
    };
    append("./file.txt", b"hello");
    append("./.git/HEAD", b"ref: refs/heads/main\n");
    append("./.git/config", b"[core]\n\trepositoryformatversion = 0\n");
    append(
        "./.git/refs/heads/main",
        format!("{}\n", "a".repeat(40)).as_bytes(),
    );
    builder.into_inner().expect("tar")
}

struct ReceiptRunner {
    ack: Option<runner_protocol::GitReceiptReadAck>,
}

impl desk_foreman::runner::RunnerService for ReceiptRunner {
    fn exec_shell<'a>(
        &'a self,
        _: runner_protocol::ExecRequest,
    ) -> desk_foreman::runner::RunnerFuture<'a, anyhow::Result<runner_protocol::ShellToolOutput>>
    {
        Box::pin(async { anyhow::bail!("unused") })
    }
    fn write_stdin<'a>(
        &'a self,
        _: runner_protocol::InputRequest,
    ) -> desk_foreman::runner::RunnerFuture<'a, anyhow::Result<runner_protocol::ShellToolOutput>>
    {
        Box::pin(async { anyhow::bail!("unused") })
    }
    fn cancel_session<'a>(
        &'a self,
        _: runner_protocol::CancelSessionRequest,
    ) -> desk_foreman::runner::RunnerFuture<'a, anyhow::Result<runner_protocol::RunnerSessionStatus>>
    {
        Box::pin(async { anyhow::bail!("unused") })
    }
    fn list_sessions<'a>(
        &'a self,
    ) -> desk_foreman::runner::RunnerFuture<
        'a,
        anyhow::Result<Vec<runner_protocol::RunnerSessionStatus>>,
    > {
        Box::pin(async { anyhow::bail!("unused") })
    }
    fn run_command<'a>(
        &'a self,
        _: runner_protocol::RunnerCommandRequest,
    ) -> desk_foreman::runner::RunnerFuture<'a, anyhow::Result<runner_protocol::CommandOutput>>
    {
        Box::pin(async { anyhow::bail!("unused") })
    }
    fn cleanup_runner_owner<'a>(
        &'a self,
        _: runner_protocol::RunnerOwner,
    ) -> desk_foreman::runner::RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async { anyhow::bail!("unused") })
    }
    fn read_git_receipt<'a>(
        &'a self,
        _: runner_protocol::GitReceiptReadRequest,
    ) -> desk_foreman::runner::RunnerFuture<'a, anyhow::Result<runner_protocol::GitReceiptReadAck>>
    {
        let ack = self.ack.clone();
        Box::pin(
            async move { ack.ok_or_else(|| anyhow::anyhow!("git receipt read is unavailable")) },
        )
    }
}

fn receipt_request() -> runner_protocol::GitReceiptReadRequest {
    runner_protocol::GitReceiptReadRequest {
        job_id: Uuid::new_v4(),
        operation_id: Uuid::new_v4(),
        assigned_manager_id: 9,
        owner: RunnerOwner::ApplicationPrincipal {
            application_id: 1,
            workspace_binding_id: 2,
            principal_digest: "a".repeat(64),
        },
        canonical_branch: "main".to_owned(),
        expected_base: None,
        workspace_root: "/srv/workspaces/binding-2".to_owned(),
        deadline_unix_ms: now_ms() + 60_000,
    }
}

fn receipt_ack(
    request: &runner_protocol::GitReceiptReadRequest,
) -> runner_protocol::GitReceiptReadAck {
    runner_protocol::GitReceiptReadAck {
        job_id: request.job_id,
        operation_id: request.operation_id,
        workspace_binding_id: 2,
        state: runner_protocol::GitReceiptState::Confirmed,
        phase: Some(runner_protocol::GitReceiptPhase::Completed),
        base: None,
        head: Some("a".repeat(40)),
        published: true,
        cleanup_confirmed: true,
    }
}

#[tokio::test]
async fn read_git_receipt_dispatch_is_strict_and_fixed_on_failure() {
    let request = receipt_request();
    let runner = ReceiptRunner {
        ack: Some(receipt_ack(&request)),
    };
    let job = runner_protocol::RunnerJob {
        job_id: "read-receipt-test".to_owned(),
        kind: "read_git_receipt".to_owned(),
        payload: serde_json::to_value(&request).expect("payload"),
    };
    let result = super::dispatch::read_git_receipt_job(&runner, job).await;
    assert!(result.ok);
    let ack: runner_protocol::GitReceiptReadAck =
        serde_json::from_value(result.result.expect("ack")).expect("ack json");
    assert!(ack.confirms(&request));

    let mismatched = ReceiptRunner {
        ack: Some(runner_protocol::GitReceiptReadAck {
            job_id: Uuid::new_v4(),
            ..receipt_ack(&request)
        }),
    };
    let job = runner_protocol::RunnerJob {
        job_id: "read-receipt-test".to_owned(),
        kind: "read_git_receipt".to_owned(),
        payload: serde_json::to_value(&request).expect("payload"),
    };
    let result = super::dispatch::read_git_receipt_job(&mismatched, job).await;
    assert!(!result.ok);
    assert_eq!(
        result.error.as_deref(),
        Some("git receipt read is unconfirmed")
    );

    let unavailable = ReceiptRunner { ack: None };
    let job = runner_protocol::RunnerJob {
        job_id: "read-receipt-test".to_owned(),
        kind: "read_git_receipt".to_owned(),
        payload: serde_json::to_value(&request).expect("payload"),
    };
    let result = super::dispatch::read_git_receipt_job(&unavailable, job).await;
    assert!(!result.ok);
    assert_eq!(
        result.error.as_deref(),
        Some("git receipt read unavailable")
    );

    let malformed = runner_protocol::RunnerJob {
        job_id: "read-receipt-test".to_owned(),
        kind: "read_git_receipt".to_owned(),
        payload: serde_json::json!({"path": "/tmp/receipt.json"}),
    };
    let result = super::dispatch::read_git_receipt_job(&unavailable, malformed).await;
    assert!(!result.ok);
    assert_eq!(
        result.error.as_deref(),
        Some("git receipt read unavailable")
    );

    let local = crate::runtime::LocalRunnerService::new(
        crate::runtime::DirectRunnerBackend::new(),
        Arc::new(tokio::sync::RwLock::new(config(private_root().path()))),
    );
    let job = runner_protocol::RunnerJob {
        job_id: "read-receipt-test".to_owned(),
        kind: "read_git_receipt".to_owned(),
        payload: serde_json::to_value(&request).expect("payload"),
    };
    let result = super::dispatch::read_git_receipt_job(local.as_ref(), job).await;
    assert!(!result.ok);
    assert_eq!(
        result.error.as_deref(),
        Some("git receipt read unavailable")
    );
}
