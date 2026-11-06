use std::collections::VecDeque;
use std::fs;
use std::future::Future;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use runner_protocol::{GitObjectFormat, GitVerifyRequest, GitVerifyResult, RunnerOwner};
use tempfile::TempDir;
use uuid::Uuid;

use super::docker_runner::{DockerCliExecutor, DockerTrustedGitRunner};
use super::helper::{GitVerifyLimits, HelperPaths, fixed_environment};
use super::import::{RawGitSnapshot, RawGitSnapshotRequest, stage_raw_git_snapshot};
use super::limits::GitImportLimits;
use super::verify::{
    GitVerifyError, TrustedGitCommand, TrustedGitFuture, TrustedGitInvocation, TrustedGitOutput,
    TrustedGitRunner, TrustedGitRunnerError, verify_raw_git_snapshot,
};

const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const OTHER: &str = "fedcba9876543210fedcba9876543210fedcba98";
const SNAPSHOT_NAME: &str = "raw-git-1-a-b-0";

fn open_dir(path: &Path) -> OwnedFd {
    fs::File::open(path).expect("open dir").into()
}

fn limits() -> GitVerifyLimits {
    GitVerifyLimits {
        timeout: Duration::from_secs(600),
        max_output_bytes: 262_144,
        memory_limit: "1g".to_owned(),
        cpu_limit: "2".to_owned(),
        pids_limit: 256,
        image: format!("runner@sha256:{}", "a".repeat(64)),
    }
}

fn owner(binding: i64) -> RunnerOwner {
    RunnerOwner::ApplicationPrincipal {
        application_id: 7,
        workspace_binding_id: binding,
        principal_digest: "principal-digest".to_owned(),
    }
}

fn request(
    head: &str,
    branch: &str,
    format: GitObjectFormat,
    base: Option<&str>,
) -> GitVerifyRequest {
    GitVerifyRequest {
        owner: owner(11),
        workspace_binding_id: 11,
        code_project_id: Uuid::new_v4(),
        authorization_generation: Uuid::new_v4(),
        expected_base: base.map(str::to_owned),
        expected_new_head: head.to_owned(),
        branch: branch.to_owned(),
        object_format: format,
        snapshot_name: SNAPSHOT_NAME.to_owned(),
    }
}

struct FakeSnapshot {
    _temp: TempDir,
    quarantine: PathBuf,
    raw: RawGitSnapshot,
    request: GitVerifyRequest,
}

impl FakeSnapshot {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let quarantine = temp.path().join("quarantine");
        fs::create_dir_all(&quarantine).expect("quarantine");
        fs::set_permissions(&quarantine, fs::Permissions::from_mode(0o700))
            .expect("quarantine mode");
        let snapshot = quarantine.join(SNAPSHOT_NAME);
        fs::create_dir_all(snapshot.join("objects")).expect("objects");
        fs::create_dir_all(snapshot.join("refs/heads")).expect("refs");
        fs::write(snapshot.join("HEAD"), b"ref: refs/heads/main\n").expect("HEAD");
        fs::write(snapshot.join("refs/heads/main"), format!("{HEAD}\n")).expect("ref");
        let raw = RawGitSnapshot {
            snapshot_name: SNAPSHOT_NAME.to_owned(),
            object_files: 0,
            object_bytes: 0,
            metadata_files: 2,
            metadata_bytes: 62,
            captured_head: HEAD.to_owned(),
            pinned_branch: "main".to_owned(),
        };
        let request = request(HEAD, "main", GitObjectFormat::Sha1, None);
        Self {
            _temp: temp,
            quarantine,
            raw,
            request,
        }
    }

    fn with_pack(mut self, bytes: &[u8]) -> Self {
        let snapshot = self.quarantine.join(SNAPSHOT_NAME);
        fs::create_dir_all(snapshot.join("objects/pack")).expect("pack");
        fs::write(
            snapshot
                .join("objects/pack")
                .join(format!("pack-{}.pack", "c".repeat(40))),
            bytes,
        )
        .expect("pack file");
        self.raw.object_files = 1;
        self.raw.object_bytes = bytes.len() as u64;
        self
    }

    fn snapshot_path(&self) -> PathBuf {
        self.quarantine.join(SNAPSHOT_NAME)
    }
}

fn ok() -> TrustedGitOutput {
    TrustedGitOutput {
        exit_code: 0,
        stdout: Vec::new(),
        stderr: Vec::new(),
    }
}

fn output(exit_code: i32, stdout: &[u8]) -> TrustedGitOutput {
    TrustedGitOutput {
        exit_code,
        stdout: stdout.to_vec(),
        stderr: Vec::new(),
    }
}

struct FakeRunner {
    results: VecDeque<Result<TrustedGitOutput, TrustedGitRunnerError>>,
    seen: Vec<TrustedGitCommand>,
    plans: Vec<(Duration, usize)>,
    init_escape: Option<PathBuf>,
}

impl FakeRunner {
    fn new(
        results: impl IntoIterator<Item = Result<TrustedGitOutput, TrustedGitRunnerError>>,
    ) -> Self {
        Self {
            results: results.into_iter().collect(),
            seen: Vec::new(),
            plans: Vec::new(),
            init_escape: None,
        }
    }

    fn escaping_pack(outside: PathBuf) -> Self {
        let mut runner = Self::new([Ok(ok())]);
        runner.init_escape = Some(outside);
        runner
    }

    fn success(base: bool) -> Self {
        let mut results = vec![Ok(ok()), Ok(output(0, b"sha1\n")), Ok(ok()), Ok(ok())];
        if base {
            results.push(Ok(ok()));
            results.push(Ok(ok()));
        }
        results.push(Ok(output(0, format!("{HEAD}\n").as_bytes())));
        Self::new(results)
    }

    fn push(&mut self, result: Result<TrustedGitOutput, TrustedGitRunnerError>) {
        self.results.push_back(result);
    }
}

impl TrustedGitRunner for FakeRunner {
    fn run<'a>(&'a mut self, invocation: TrustedGitInvocation) -> TrustedGitFuture<'a> {
        Box::pin(async move {
            if matches!(invocation.command, TrustedGitCommand::Init { .. }) {
                fs::create_dir_all(invocation.paths.repo_dir.join("objects")).expect("objects");
                fs::create_dir_all(invocation.paths.repo_dir.join("objects/info")).expect("info");
                match &self.init_escape {
                    Some(outside) => {
                        symlink(outside, invocation.paths.repo_dir.join("objects/pack"))
                            .expect("escaping pack symlink");
                    }
                    None => {
                        fs::create_dir_all(invocation.paths.repo_dir.join("objects/pack"))
                            .expect("pack");
                    }
                }
            }
            self.plans.push((
                invocation.plan.deadline(),
                invocation.plan.max_output_bytes(),
            ));
            self.seen.push(invocation.command);
            self.results.pop_front().unwrap_or_else(|| Ok(ok()))
        })
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
        .block_on(future)
}

struct PendingRunner;

impl TrustedGitRunner for PendingRunner {
    fn run<'a>(&'a mut self, _invocation: TrustedGitInvocation) -> TrustedGitFuture<'a> {
        Box::pin(async { std::future::pending().await })
    }
}

fn retained_operation_dirs(quarantine: &Path) -> Vec<PathBuf> {
    fs::read_dir(quarantine)
        .expect("quarantine")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("git-verify-"))
        })
        .collect()
}

fn run_verify(
    snapshot: &FakeSnapshot,
    runner: &mut FakeRunner,
) -> Result<GitVerifyResult, GitVerifyError> {
    let quarantine = open_dir(&snapshot.quarantine);
    block_on(verify_raw_git_snapshot(
        &quarantine,
        &snapshot.raw,
        &snapshot.request,
        &limits(),
        runner,
    ))
    .map(|artifact| artifact.summary().clone())
}

#[test]
fn fake_runner_success_produces_verified_result_and_retains_snapshot() {
    let snapshot = FakeSnapshot::new();
    let mut runner = FakeRunner::success(false);
    let result = run_verify(&snapshot, &mut runner).expect("verified");
    assert_eq!(
        result,
        GitVerifyResult {
            head: HEAD.to_owned(),
            branch: "main".to_owned(),
            base: None,
            object_format: GitObjectFormat::Sha1,
        }
    );
    assert_eq!(
        runner.seen,
        vec![
            TrustedGitCommand::Init {
                object_format: GitObjectFormat::Sha1,
                initial_branch: "main".to_owned(),
            },
            TrustedGitCommand::ShowObjectFormat,
            TrustedGitCommand::Fsck,
            TrustedGitCommand::CatFileCommit {
                head: HEAD.to_owned()
            },
            TrustedGitCommand::RevParseCommit {
                head: HEAD.to_owned()
            },
        ]
    );
    assert!(snapshot.snapshot_path().join("HEAD").exists());
    let entries: Vec<_> = fs::read_dir(&snapshot.quarantine)
        .expect("quarantine")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(entries, vec![std::ffi::OsString::from(SNAPSHOT_NAME)]);
}

#[test]
fn fake_steps_use_remaining_deadline_and_shared_output_quota() {
    let snapshot = FakeSnapshot::new();
    let mut runner = FakeRunner::success(false);
    run_verify(&snapshot, &mut runner).expect("verified");
    assert!(runner.plans.len() >= 5);
    assert!(
        runner.plans.windows(2).all(|pair| pair[1].0 <= pair[0].0),
        "step deadlines must not reset"
    );
    assert_eq!(runner.plans[0].1, limits().max_output_bytes);
    assert!(
        runner.plans.windows(2).all(|pair| pair[1].1 <= pair[0].1),
        "step output quota must subtract earlier output"
    );
    assert!(
        runner.plans.last().expect("last plan").1 < limits().max_output_bytes,
        "shared quota must shrink after observed output"
    );
}

#[test]
fn fast_forward_sequence_runs_ancestry_with_requested_base() {
    let mut snapshot = FakeSnapshot::new();
    snapshot.request.expected_base = Some(OTHER.to_owned());
    let mut runner = FakeRunner::success(true);
    let result = run_verify(&snapshot, &mut runner).expect("fast-forward");
    assert_eq!(result.base.as_deref(), Some(OTHER));
    assert!(
        runner
            .seen
            .contains(&TrustedGitCommand::MergeBaseIsAncestor {
                base: OTHER.to_owned(),
                head: HEAD.to_owned(),
            })
    );
}

#[test]
fn detached_head_snapshot_is_accepted() {
    let snapshot = FakeSnapshot::new();
    let root = snapshot.snapshot_path();
    fs::remove_dir_all(root.join("refs")).expect("remove refs");
    fs::write(root.join("HEAD"), format!("{HEAD}\n")).expect("detached HEAD");
    let mut runner = FakeRunner::success(false);
    let result = run_verify(&snapshot, &mut runner).expect("detached head");
    assert_eq!(result.head, HEAD);
}

#[test]
fn branch_with_slash_validates_nested_ref_layout() {
    let mut snapshot = FakeSnapshot::new();
    let root = snapshot.snapshot_path();
    fs::remove_dir_all(root.join("refs")).expect("remove refs");
    fs::create_dir_all(root.join("refs/heads/feature")).expect("feature");
    fs::write(root.join("refs/heads/feature/x"), format!("{HEAD}\n")).expect("ref");
    fs::write(root.join("HEAD"), b"ref: refs/heads/feature/x\n").expect("HEAD");
    snapshot.raw.pinned_branch = "feature/x".to_owned();
    snapshot.request.branch = "feature/x".to_owned();
    let mut runner = FakeRunner::success(false);
    let result = run_verify(&snapshot, &mut runner).expect("slash branch");
    assert_eq!(result.branch, "feature/x");
}

#[test]
fn destination_symlink_escape_is_rejected_without_touching_outside() {
    let snapshot = FakeSnapshot::new().with_pack(b"pack");
    let outside = snapshot.quarantine.join("outside-destination");
    fs::create_dir_all(&outside).expect("outside");
    fs::write(outside.join("canary"), b"keep").expect("canary");
    let mut runner = FakeRunner::escaping_pack(outside.clone());
    assert_eq!(
        run_verify(&snapshot, &mut runner),
        Err(GitVerifyError::Preparation)
    );
    assert_eq!(
        fs::read(outside.join("canary")).expect("canary read"),
        b"keep"
    );
    assert!(!outside.join("pack").exists());
}

#[test]
fn deterministic_runner_errors_map_to_fixed_classes() {
    let snapshot = FakeSnapshot::new();

    let mut runner = FakeRunner::new([Err(TrustedGitRunnerError::TimedOut)]);
    assert_eq!(
        run_verify(&snapshot, &mut runner),
        Err(GitVerifyError::Timeout)
    );

    let mut runner = FakeRunner::new([Err(TrustedGitRunnerError::Cancelled)]);
    assert_eq!(
        run_verify(&snapshot, &mut runner),
        Err(GitVerifyError::Cancelled)
    );

    let mut runner = FakeRunner::new([Err(TrustedGitRunnerError::Failed)]);
    assert_eq!(
        run_verify(&snapshot, &mut runner),
        Err(GitVerifyError::RunnerFailed)
    );

    let mut snapshot = FakeSnapshot::new();
    snapshot.request.expected_base = Some(OTHER.to_owned());
    let mut results = vec![
        Ok(ok()),
        Ok(output(0, b"sha1\n")),
        Ok(ok()),
        Ok(ok()),
        Ok(ok()),
        Ok(output(1, b"")),
    ];
    let mut runner = FakeRunner::new(results.drain(..));
    assert_eq!(
        run_verify(&snapshot, &mut runner),
        Err(GitVerifyError::NonFastForward)
    );

    let mut snapshot = FakeSnapshot::new();
    snapshot.request.expected_base = Some(OTHER.to_owned());
    let mut results = vec![
        Ok(ok()),
        Ok(output(0, b"sha1\n")),
        Ok(ok()),
        Ok(ok()),
        Ok(output(1, b"")),
    ];
    let mut runner = FakeRunner::new(results.drain(..));
    assert_eq!(
        run_verify(&snapshot, &mut runner),
        Err(GitVerifyError::MissingCommit)
    );

    let mut snapshot = FakeSnapshot::new();
    snapshot.request.expected_base = Some(OTHER.to_owned());
    let mut results = vec![
        Ok(ok()),
        Ok(output(0, b"sha1\n")),
        Ok(ok()),
        Ok(ok()),
        Ok(ok()),
        Ok(output(2, b"ancestry error")),
    ];
    let mut runner = FakeRunner::new(results.drain(..));
    assert_eq!(
        run_verify(&snapshot, &mut runner),
        Err(GitVerifyError::AncestryFailed)
    );

    let snapshot = FakeSnapshot::new();
    let mut results = vec![Ok(ok()), Ok(output(0, b"sha256\n"))];
    let mut runner = FakeRunner::new(results.drain(..));
    assert_eq!(
        run_verify(&snapshot, &mut runner),
        Err(GitVerifyError::ObjectFormatMismatch)
    );

    let snapshot = FakeSnapshot::new();
    let mut results = vec![
        Ok(ok()),
        Ok(output(0, b"sha1\n")),
        Ok(output(2, b"corrupt")),
    ];
    let mut runner = FakeRunner::new(results.drain(..));
    assert_eq!(
        run_verify(&snapshot, &mut runner),
        Err(GitVerifyError::FsckFailed)
    );

    let snapshot = FakeSnapshot::new();
    let mut results = vec![
        Ok(ok()),
        Ok(output(0, b"sha1\n")),
        Ok(ok()),
        Ok(output(1, b"")),
    ];
    let mut runner = FakeRunner::new(results.drain(..));
    assert_eq!(
        run_verify(&snapshot, &mut runner),
        Err(GitVerifyError::MissingCommit)
    );

    let snapshot = FakeSnapshot::new();
    let mut results = vec![
        Ok(ok()),
        Ok(output(0, b"sha1\n")),
        Ok(ok()),
        Ok(ok()),
        Ok(output(0, format!("{OTHER}\n").as_bytes())),
    ];
    let mut runner = FakeRunner::new(results.drain(..));
    assert_eq!(
        run_verify(&snapshot, &mut runner),
        Err(GitVerifyError::HeadMismatch)
    );
}

#[test]
fn output_cap_is_operation_wide() {
    let snapshot = FakeSnapshot::new();
    let mut runner = FakeRunner::new([Ok(output(0, b"0123456789"))]);
    let quarantine = open_dir(&snapshot.quarantine);
    let mut small = limits();
    small.max_output_bytes = 9;
    assert!(matches!(
        block_on(verify_raw_git_snapshot(
            &quarantine,
            &snapshot.raw,
            &snapshot.request,
            &small,
            &mut runner
        )),
        Err(GitVerifyError::OutputLimit)
    ));
    assert_eq!(runner.seen.len(), 1);
}

#[test]
fn output_cap_accumulates_across_the_sequence() {
    let snapshot = FakeSnapshot::new();
    let mut runner = FakeRunner::new([Ok(output(0, b"12345")), Ok(output(0, b"67890"))]);
    let quarantine = open_dir(&snapshot.quarantine);
    let mut small = limits();
    small.max_output_bytes = 8;
    assert!(matches!(
        block_on(verify_raw_git_snapshot(
            &quarantine,
            &snapshot.raw,
            &snapshot.request,
            &small,
            &mut runner
        )),
        Err(GitVerifyError::OutputLimit)
    ));
    assert_eq!(runner.seen.len(), 2);
}

#[test]
fn request_and_snapshot_mismatches_reject_before_runner() {
    let mut snapshot = FakeSnapshot::new();
    snapshot.request.expected_new_head = OTHER.to_owned();
    let mut runner = FakeRunner::success(false);
    assert_eq!(
        run_verify(&snapshot, &mut runner),
        Err(GitVerifyError::SnapshotMismatch)
    );
    assert!(runner.seen.is_empty());

    let mut snapshot = FakeSnapshot::new();
    snapshot.request.snapshot_name = "fixed-snapshot".to_owned();
    let mut runner = FakeRunner::success(false);
    assert_eq!(
        run_verify(&snapshot, &mut runner),
        Err(GitVerifyError::InvalidRequest)
    );
    assert!(runner.seen.is_empty());
}

#[test]
fn cleanup_and_reap_uncertainty_retain_only_the_operation_directory() {
    for error in [
        TrustedGitRunnerError::Cleanup,
        TrustedGitRunnerError::ReapTimeout,
    ] {
        let snapshot = FakeSnapshot::new();
        let mut runner = FakeRunner::new([Err(error)]);
        assert!(run_verify(&snapshot, &mut runner).is_err());
        let retained = retained_operation_dirs(&snapshot.quarantine);
        assert_eq!(retained.len(), 1);
        assert!(retained[0].join("repo").is_dir());
        assert!(snapshot.snapshot_path().join("HEAD").exists());
        fs::remove_dir_all(&retained[0]).expect("test cleanup retained dir");
        assert!(retained_operation_dirs(&snapshot.quarantine).is_empty());
    }
}

#[tokio::test]
async fn aborting_while_a_fake_helper_is_live_retains_the_directory() {
    let snapshot = FakeSnapshot::new();
    let quarantine = open_dir(&snapshot.quarantine);
    let mut runner = PendingRunner;
    let result = tokio::time::timeout(
        Duration::from_millis(50),
        verify_raw_git_snapshot(
            &quarantine,
            &snapshot.raw,
            &snapshot.request,
            &limits(),
            &mut runner,
        ),
    )
    .await;
    assert!(result.is_err(), "pending helper should time out");
    let retained = retained_operation_dirs(&snapshot.quarantine);
    assert_eq!(retained.len(), 1);
    fs::remove_dir_all(&retained[0]).expect("test cleanup retained dir");
    assert!(snapshot.snapshot_path().join("HEAD").exists());
}

#[test]
fn snapshot_layout_rejects_links_and_unexpected_entries_before_runner() {
    let snapshot = FakeSnapshot::new();
    let outside = snapshot.quarantine.join("outside");
    fs::write(&outside, b"outside").expect("outside");
    let objects = snapshot.snapshot_path().join("objects");
    fs::remove_dir(&objects).expect("remove objects");
    symlink(&outside, &objects).expect("symlink objects");
    let mut runner = FakeRunner::success(false);
    assert_eq!(
        run_verify(&snapshot, &mut runner),
        Err(GitVerifyError::SnapshotLayout)
    );
    assert!(runner.seen.is_empty());
    assert_eq!(fs::read(&outside).expect("outside read"), b"outside");

    let snapshot = FakeSnapshot::new();
    fs::write(snapshot.snapshot_path().join("config"), b"canary").expect("config");
    let mut runner = FakeRunner::success(false);
    assert_eq!(
        run_verify(&snapshot, &mut runner),
        Err(GitVerifyError::SnapshotLayout)
    );
    assert!(runner.seen.is_empty());

    let snapshot = FakeSnapshot::new();
    let head = snapshot.snapshot_path().join("HEAD");
    fs::remove_file(&head).expect("remove HEAD");
    symlink(&outside, &head).expect("symlink HEAD");
    let mut runner = FakeRunner::success(false);
    assert_eq!(
        run_verify(&snapshot, &mut runner),
        Err(GitVerifyError::SnapshotLayout)
    );
    assert!(runner.seen.is_empty());
}

#[test]
fn hardlinked_object_is_rejected_before_copy() {
    let snapshot = FakeSnapshot::new().with_pack(b"pack");
    let outside = snapshot.quarantine.join("outside-hardlink");
    fs::write(&outside, b"outside").expect("outside");
    let pack = snapshot
        .snapshot_path()
        .join("objects/pack")
        .join(format!("pack-{}.pack", "c".repeat(40)));
    fs::remove_file(&pack).expect("remove pack");
    fs::hard_link(&outside, &pack).expect("hardlink pack");
    let mut runner = FakeRunner::success(false);
    assert_eq!(
        run_verify(&snapshot, &mut runner),
        Err(GitVerifyError::SnapshotLayout)
    );
    assert_eq!(runner.seen.len(), 1);
    assert_eq!(fs::read(&outside).expect("outside read"), b"outside");
}

#[test]
fn corrupt_pack_is_copied_raw_and_fails_through_fixed_fsck_error_only() {
    let snapshot = FakeSnapshot::new().with_pack(b"not a real pack");
    let mut runner = FakeRunner::new([
        Ok(ok()),
        Ok(output(0, b"sha1\n")),
        Ok(output(2, b"fsck rejected")),
    ]);
    assert_eq!(
        run_verify(&snapshot, &mut runner),
        Err(GitVerifyError::FsckFailed)
    );
    assert_eq!(runner.seen.len(), 3);
}

#[test]
fn cleanup_removes_only_the_operation_directory() {
    let snapshot = FakeSnapshot::new();
    let outside = snapshot.quarantine.join("outside-canary");
    fs::write(&outside, b"keep").expect("canary");
    let mut runner = FakeRunner::new([Err(TrustedGitRunnerError::Failed)]);
    assert_eq!(
        run_verify(&snapshot, &mut runner),
        Err(GitVerifyError::RunnerFailed)
    );
    assert_eq!(fs::read(&outside).expect("canary read"), b"keep");
    assert!(snapshot.snapshot_path().join("HEAD").exists());
    let mut names: Vec<_> = fs::read_dir(&snapshot.quarantine)
        .expect("quarantine")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            std::ffi::OsString::from("outside-canary"),
            std::ffi::OsString::from(SNAPSHOT_NAME)
        ]
    );
}

struct HostTrustedGitRunner;

impl TrustedGitRunner for HostTrustedGitRunner {
    fn run<'a>(&'a mut self, invocation: TrustedGitInvocation) -> TrustedGitFuture<'a> {
        Box::pin(async move {
            let mut command = host_command(&invocation.paths, &invocation.command);
            let output = command
                .output()
                .map_err(|_| TrustedGitRunnerError::Failed)?;
            Ok(TrustedGitOutput {
                exit_code: output.status.code().unwrap_or(-1),
                stdout: output.stdout,
                stderr: output.stderr,
            })
        })
    }
}

fn host_command(paths: &HelperPaths, command: &TrustedGitCommand) -> Command {
    let mut process = Command::new("git");
    process.env_clear();
    for &(name, value) in fixed_environment() {
        let value = if name == std::ffi::OsStr::new("HOME") {
            paths.home_dir.to_string_lossy().into_owned()
        } else {
            value.to_owned()
        };
        process.env(name, value);
    }
    process.current_dir(&paths.operation_root);
    match command_for(paths, command) {
        Ok(arguments) => {
            process.args(arguments);
        }
        Err(error) => panic!("host command plan: {error}"),
    }
    process
}

fn command_for(
    paths: &HelperPaths,
    command: &TrustedGitCommand,
) -> Result<Vec<String>, &'static str> {
    let repo = paths.repo_dir.to_string_lossy().into_owned();
    let template = paths.template_dir.to_string_lossy().into_owned();
    let arguments = match command {
        TrustedGitCommand::Init {
            object_format,
            initial_branch,
        } => vec![
            "init".to_owned(),
            "--bare".to_owned(),
            format!("--object-format={}", object_format.git_name()),
            format!("--initial-branch={initial_branch}"),
            "--ref-format=files".to_owned(),
            format!("--template={template}"),
            repo,
        ],
        TrustedGitCommand::ShowObjectFormat => vec![
            "-C".to_owned(),
            repo,
            "rev-parse".to_owned(),
            "--show-object-format".to_owned(),
        ],
        TrustedGitCommand::Fsck => vec![
            "-C".to_owned(),
            repo,
            "fsck".to_owned(),
            "--no-dangling".to_owned(),
            "--no-progress".to_owned(),
        ],
        TrustedGitCommand::CatFileCommit { head } => vec![
            "-C".to_owned(),
            repo,
            "cat-file".to_owned(),
            "-e".to_owned(),
            format!("{head}^{{commit}}"),
        ],
        TrustedGitCommand::MergeBaseIsAncestor { base, head } => vec![
            "-C".to_owned(),
            repo,
            "merge-base".to_owned(),
            "--is-ancestor".to_owned(),
            base.clone(),
            head.clone(),
        ],
        TrustedGitCommand::RevParseCommit { head } => vec![
            "-C".to_owned(),
            repo,
            "rev-parse".to_owned(),
            format!("{head}^{{commit}}"),
        ],
        TrustedGitCommand::ShowRef { branch } => vec![
            "-C".to_owned(),
            repo,
            "show-ref".to_owned(),
            "--verify".to_owned(),
            "--hash".to_owned(),
            format!("refs/heads/{branch}"),
        ],
        TrustedGitCommand::BundleCreate { branch } => vec![
            "-C".to_owned(),
            repo,
            "bundle".to_owned(),
            "create".to_owned(),
            "-q".to_owned(),
            "-".to_owned(),
            format!("refs/heads/{branch}"),
        ],
    };
    Ok(arguments)
}

#[test]
fn host_runner_environment_is_closed_and_has_no_credentials() {
    let paths = HelperPaths {
        operation_root: PathBuf::from("/private/op"),
        repo_dir: PathBuf::from("/private/op/repo"),
        home_dir: PathBuf::from("/private/op/home"),
        template_dir: PathBuf::from("/private/op/template"),
        uid: 1000,
        gid: 1000,
    };
    let command = host_command(&paths, &TrustedGitCommand::Fsck);
    let environment: Vec<_> = command.get_envs().collect();
    let names: Vec<String> = environment
        .iter()
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect();
    for forbidden in [
        "SSH_AUTH_SOCK",
        "GIT_DIR",
        "GIT_WORK_TREE",
        "SSH_PRIVATE_KEY",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "LD_PRELOAD",
    ] {
        assert!(!names.iter().any(|name| name == forbidden));
    }
    assert!(names.iter().any(|name| name == "HOME"));
    assert!(
        environment
            .iter()
            .any(|(name, value)| name.to_string_lossy() == "HOME"
                && value.is_some_and(|value| value == paths.home_dir.as_os_str()))
    );
    assert!(environment.iter().all(|(_, value)| value.is_some()));
}

struct RealRepo {
    temp: TempDir,
    repo: PathBuf,
    quarantine: PathBuf,
}

impl RealRepo {
    fn new(format: &str) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        git_init(&repo, format);
        let quarantine = temp.path().join("quarantine");
        fs::create_dir_all(&quarantine).expect("quarantine");
        fs::set_permissions(&quarantine, fs::Permissions::from_mode(0o700))
            .expect("quarantine mode");
        Self {
            temp,
            repo,
            quarantine,
        }
    }

    fn commit_file(&self, name: &str, contents: &str, message: &str) -> String {
        fs::write(self.repo.join(name), contents).expect("write file");
        git(&self.repo, &["add", "-A"]);
        git(&self.repo, &["commit", "-m", message]);
        self.head()
    }

    fn head(&self) -> String {
        git(&self.repo, &["rev-parse", "HEAD"])
    }

    fn branch(&self) -> String {
        git(&self.repo, &["symbolic-ref", "--short", "HEAD"])
    }

    fn stage(&self) -> RawGitSnapshot {
        let workspace = self.temp.path();
        let checkout = self.repo.strip_prefix(workspace).expect("relative repo");
        stage_raw_git_snapshot(
            RawGitSnapshotRequest {
                workspace_root: open_dir(workspace),
                quarantine_root: open_dir(&self.quarantine),
                checkout_relative: checkout.to_path_buf(),
                pinned_branch: self.branch(),
                expected_head: self.head(),
            },
            GitImportLimits {
                max_entries: 100_000,
                max_files: 100_000,
                max_file_bytes: 16 * 1024 * 1024,
                max_total_bytes: 64 * 1024 * 1024,
                max_metadata_bytes: 1024 * 1024,
            },
        )
        .expect("stage")
    }

    fn request(
        &self,
        raw: &RawGitSnapshot,
        format: GitObjectFormat,
        base: Option<&str>,
    ) -> GitVerifyRequest {
        GitVerifyRequest {
            owner: owner(11),
            workspace_binding_id: 11,
            code_project_id: Uuid::new_v4(),
            authorization_generation: Uuid::new_v4(),
            expected_base: base.map(str::to_owned),
            expected_new_head: raw.captured_head.clone(),
            branch: raw.pinned_branch.clone(),
            object_format: format,
            snapshot_name: raw.snapshot_name.clone(),
        }
    }
}

fn base_git_command() -> Command {
    let mut command = Command::new("git");
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com");
    command
}

fn git_init(repo: &Path, format: &str) {
    let output = base_git_command()
        .arg("init")
        .arg("-b")
        .arg("main")
        .arg(format!("--object-format={format}"))
        .arg(repo)
        .output()
        .expect("git init");
    assert!(output.status.success(), "git init failed: {output:?}");
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = base_git_command()
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf8")
        .trim()
        .to_owned()
}

fn verify_real(
    repo: &RealRepo,
    raw: &RawGitSnapshot,
    base: Option<&str>,
) -> Result<GitVerifyResult, GitVerifyError> {
    let request = repo.request(raw, GitObjectFormat::Sha1, base);
    let quarantine = open_dir(&repo.quarantine);
    block_on(verify_raw_git_snapshot(
        &quarantine,
        raw,
        &request,
        &limits(),
        &mut HostTrustedGitRunner,
    ))
    .map(|artifact| artifact.summary().clone())
}

#[test]
fn host_runner_verifies_benign_sha1_repository() {
    let repo = RealRepo::new("sha1");
    let head = repo.commit_file("file.txt", "hello\n", "initial");
    assert_eq!(head.len(), 40);
    let raw = repo.stage();
    let result = verify_real(&repo, &raw, None).expect("verify sha1");
    assert_eq!(result.head, head);
    assert_eq!(result.branch, "main");
    assert_eq!(result.base, None);
    assert_eq!(result.object_format, GitObjectFormat::Sha1);
}

#[test]
fn verification_artifact_captures_the_full_owner_identity() {
    let repo = RealRepo::new("sha1");
    repo.commit_file("file.txt", "hello\n", "initial");
    let raw = repo.stage();
    let request = repo.request(&raw, GitObjectFormat::Sha1, None);
    let quarantine = open_dir(&repo.quarantine);
    let artifact = block_on(verify_raw_git_snapshot(
        &quarantine,
        &raw,
        &request,
        &limits(),
        &mut HostTrustedGitRunner,
    ))
    .expect("verified artifact");
    assert!(artifact.identity().matches(&request));
    let mut changed = request.clone();
    changed.owner = RunnerOwner::ApplicationPrincipal {
        application_id: 7,
        workspace_binding_id: 11,
        principal_digest: "different-digest".to_owned(),
    };
    assert!(!artifact.identity().matches(&changed));
}

#[test]
fn owner_binding_mismatch_is_rejected_before_the_runner() {
    let repo = RealRepo::new("sha1");
    repo.commit_file("file.txt", "hello\n", "initial");
    let raw = repo.stage();
    let mut request = repo.request(&raw, GitObjectFormat::Sha1, None);
    request.workspace_binding_id = 12;
    let quarantine = open_dir(&repo.quarantine);
    assert!(matches!(
        block_on(verify_raw_git_snapshot(
            &quarantine,
            &raw,
            &request,
            &limits(),
            &mut HostTrustedGitRunner,
        )),
        Err(GitVerifyError::InvalidRequest)
    ));
}

#[test]
fn host_verification_returns_a_retained_artifact_after_raw_removal() {
    let repo = RealRepo::new("sha1");
    let head = repo.commit_file("file.txt", "hello\n", "initial");
    let raw = repo.stage();
    let request = repo.request(&raw, GitObjectFormat::Sha1, None);
    let quarantine = open_dir(&repo.quarantine);
    let artifact = block_on(verify_raw_git_snapshot(
        &quarantine,
        &raw,
        &request,
        &limits(),
        &mut HostTrustedGitRunner,
    ))
    .expect("verified artifact");
    let operation_root = artifact.operation_root().to_path_buf();
    assert!(operation_root.join("repo/objects").is_dir());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(operation_root.join("repo/config"))
            .expect("config")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(operation_root.join("repo"))
            .expect("repo")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(artifact.summary().head, head);

    fs::remove_dir_all(repo.quarantine.join(&raw.snapshot_name)).expect("remove raw snapshot");
    assert_eq!(artifact.summary().head, head);
    assert!(operation_root.join("repo/objects").is_dir());
    drop(artifact);
    assert!(!operation_root.exists());
    assert!(!repo.quarantine.join(&raw.snapshot_name).exists());
}

#[test]
fn host_runner_verifies_benign_sha256_repository() {
    let repo = RealRepo::new("sha256");
    let head = repo.commit_file("file.txt", "hello\n", "initial");
    assert_eq!(head.len(), 64);
    let raw = repo.stage();
    let request = repo.request(&raw, GitObjectFormat::Sha256, None);
    let quarantine = open_dir(&repo.quarantine);
    let result = block_on(verify_raw_git_snapshot(
        &quarantine,
        &raw,
        &request,
        &limits(),
        &mut HostTrustedGitRunner,
    ))
    .expect("verify sha256");
    assert_eq!(result.summary().head, head);
    assert_eq!(result.summary().object_format, GitObjectFormat::Sha256);
}

#[test]
fn host_runner_accepts_fast_forward_and_rejects_non_fast_forward() {
    let repo = RealRepo::new("sha1");
    let first = repo.commit_file("file.txt", "one\n", "initial");
    let second = repo.commit_file("file.txt", "two\n", "second");
    let raw = repo.stage();
    let fast_forward = verify_real(&repo, &raw, Some(&first)).expect("fast-forward");
    assert_eq!(fast_forward.head, second);
    assert_eq!(fast_forward.base.as_deref(), Some(first.as_str()));

    git(&repo.repo, &["branch", "other"]);
    git(&repo.repo, &["checkout", "other"]);
    let divergent = repo.commit_file("other.txt", "other\n", "divergent");
    git(&repo.repo, &["checkout", "main"]);
    let main = repo.commit_file("main.txt", "main\n", "mainline");
    let raw = repo.stage();
    let request = repo.request(&raw, GitObjectFormat::Sha1, Some(&divergent));
    let quarantine = open_dir(&repo.quarantine);
    assert!(matches!(
        block_on(verify_raw_git_snapshot(
            &quarantine,
            &raw,
            &request,
            &limits(),
            &mut HostTrustedGitRunner
        )),
        Err(GitVerifyError::NonFastForward)
    ));
    assert_eq!(main, repo.head());
}

#[test]
fn host_runner_keeps_attributes_and_gitlinks_inert() {
    let repo = RealRepo::new("sha1");
    fs::write(repo.repo.join(".gitattributes"), "* text=auto\n").expect("attributes");
    symlink("file.txt", repo.repo.join("tracked-link")).expect("tracked symlink");
    let first = repo.commit_file("file.txt", "one\n", "initial");
    git(
        &repo.repo,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{first},submodule"),
        ],
    );
    git(&repo.repo, &["commit", "-m", "gitlink"]);
    let head = repo.head();
    let raw = repo.stage();
    let result = verify_real(&repo, &raw, None).expect("verify attributes and gitlink");
    assert_eq!(result.head, head);
    assert!(!repo.repo.join("submodule").exists());
}

fn docker_test_guard() -> (String, String) {
    if std::env::var("DF_GIT_HELPER_TEST_ALLOW").as_deref() != Ok("1") {
        panic!("DF_GIT_HELPER_TEST_ALLOW=1 is required; refusing to run");
    }
    let image = std::env::var("DF_GIT_HELPER_TEST_IMAGE")
        .expect("DF_GIT_HELPER_TEST_IMAGE digest-pinned helper image is required");
    let host = std::env::var("DF_GIT_HELPER_TEST_DOCKER_HOST")
        .expect("DF_GIT_HELPER_TEST_DOCKER_HOST dedicated test daemon is required");
    let isolation = std::env::var("DF_GIT_HELPER_TEST_OPERATOR_ISOLATION")
        .expect("DF_GIT_HELPER_TEST_OPERATOR_ISOLATION=1 is required");
    assert_eq!(
        isolation, "1",
        "operator-approved isolation marker required"
    );
    assert!(
        image.contains("@sha256:"),
        "explicit digest-pinned helper image required"
    );
    (image, host)
}

fn docker_runner(_image: String, host: String) -> DockerTrustedGitRunner<DockerCliExecutor> {
    let executor = DockerCliExecutor::new("docker".to_owned(), Some(host.clone()))
        .expect("dedicated docker CLI/host");
    DockerTrustedGitRunner::new(executor, "docker".to_owned(), Some(host), None)
        .expect("docker runner")
}

#[test]
#[ignore = "requires DF_GIT_HELPER_TEST_ALLOW=1, a digest-pinned helper image, a dedicated test daemon, and the operator isolation marker"]
fn ignored_real_container_benign_sha1_verification() {
    let (image, host) = docker_test_guard();
    let repo = RealRepo::new("sha1");
    let head = repo.commit_file("file.txt", "hello\n", "initial");
    let raw = repo.stage();
    let request = repo.request(&raw, GitObjectFormat::Sha1, None);
    let quarantine = open_dir(&repo.quarantine);
    let mut docker_limits = limits();
    docker_limits.image = image;
    let mut runner = docker_runner(docker_limits.image.clone(), host);
    let artifact = block_on(verify_raw_git_snapshot(
        &quarantine,
        &raw,
        &request,
        &docker_limits,
        &mut runner,
    ))
    .expect("real container verification");
    assert_eq!(artifact.summary().head, head);
}

#[test]
#[ignore = "requires DF_GIT_HELPER_TEST_ALLOW=1, a digest-pinned helper image, a dedicated test daemon, and the operator isolation marker"]
fn ignored_real_container_adversarial_canaries_remain_inert() {
    let (image, host) = docker_test_guard();
    let repo = RealRepo::new("sha1");
    repo.commit_file("file.txt", "hello\n", "initial");
    let marker = repo.temp.path().join("real-container-hook");
    let hook = repo.repo.join(".git/hooks/pre-commit");
    fs::write(&hook, format!("#!/bin/sh\n: > '{}'\n", marker.display())).expect("hook");
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).expect("hook mode");
    let config = repo.repo.join(".git/config");
    let mut config_text = fs::read_to_string(&config).expect("config");
    config_text.push_str(
        "[core]\n\tfsmonitor = /nonexistent/canary\n[url \"ssh://canary\"]\n\tinsteadOf = https://canary\n[http]\n\tproxy = http://canary\n",
    );
    fs::write(&config, config_text).expect("config");
    let raw = repo.stage();
    let request = repo.request(&raw, GitObjectFormat::Sha1, None);
    let quarantine = open_dir(&repo.quarantine);
    let mut docker_limits = limits();
    docker_limits.image = image;
    let mut runner = docker_runner(docker_limits.image.clone(), host);
    block_on(verify_raw_git_snapshot(
        &quarantine,
        &raw,
        &request,
        &docker_limits,
        &mut runner,
    ))
    .expect("real container verification");
    assert!(!marker.exists(), "config/hook canary must never execute");
}

#[test]
#[ignore = "requires DF_GIT_HELPER_TEST_ALLOW=1, a digest-pinned helper image, a dedicated test daemon, and the operator isolation marker"]
fn ignored_real_container_malicious_pack_stays_bounded() {
    let (image, host) = docker_test_guard();
    let snapshot = FakeSnapshot::new().with_pack(&vec![0_u8; 1024 * 1024]);
    let quarantine = open_dir(&snapshot.quarantine);
    let mut docker_limits = limits();
    docker_limits.image = image;
    let mut runner = docker_runner(docker_limits.image.clone(), host);
    assert!(matches!(
        block_on(verify_raw_git_snapshot(
            &quarantine,
            &snapshot.raw,
            &snapshot.request,
            &docker_limits,
            &mut runner,
        )),
        Err(GitVerifyError::FsckFailed)
    ));
}

#[test]
fn malicious_repository_canaries_are_not_copied_or_executed() {
    let repo = RealRepo::new("sha1");
    repo.commit_file("file.txt", "hello\n", "initial");
    let marker = repo.temp.path().join("hook-executed");
    let hook = repo.repo.join(".git/hooks/pre-commit");
    fs::write(&hook, format!("#!/bin/sh\n: > '{}'\n", marker.display())).expect("hook");
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).expect("hook mode");
    let config = repo.repo.join(".git/config");
    let mut config_text = fs::read_to_string(&config).expect("config");
    config_text.push_str(
        "[core]\n\tfsmonitor = /nonexistent/canary\n[url \"ssh://canary\"]\n\tinsteadOf = https://canary\n[http]\n\tproxy = http://canary\n",
    );
    fs::write(&config, config_text).expect("config");
    let raw = repo.stage();
    let snapshot = repo.quarantine.join(&raw.snapshot_name);
    assert!(!snapshot.join("config").exists());
    assert!(!snapshot.join("hooks").exists());
    verify_real(&repo, &raw, None).expect("verify benign objects");
    assert!(!marker.exists(), "model hook must never execute");
}
