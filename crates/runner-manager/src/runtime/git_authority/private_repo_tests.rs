use std::collections::VecDeque;
use std::fs;
use std::future::Future;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use runner_protocol::{GitObjectFormat, GitVerifyRequest, GitVerifyResult, RunnerOwner};
use tempfile::{TempDir, tempdir};
use uuid::Uuid;

use super::artifact::{VerifiedGitIdentity, VerifiedGitRepository};
use super::docker_runner::{DockerCliExecutor, DockerTrustedGitRunner};
use super::helper::{GitVerifyLimits, HelperPaths, fixed_environment};
use super::import::{RawGitSnapshot, RawGitSnapshotRequest, stage_raw_git_snapshot};
use super::limits::GitImportLimits;
use super::private_repo::{
    GitExportLimit, GitStageError, MAX_EXPORT_BUNDLE_BYTES, stage_verified_bundle,
    write_verified_ref,
};
use super::verify::{
    TrustedGitCommand, TrustedGitFuture, TrustedGitInvocation, TrustedGitOutput, TrustedGitRunner,
    TrustedGitRunnerError, verify_raw_git_snapshot,
};

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

fn request(head: &str, branch: &str, format: GitObjectFormat) -> GitVerifyRequest {
    GitVerifyRequest {
        owner: owner(11),
        workspace_binding_id: 11,
        code_project_id: Uuid::new_v4(),
        authorization_generation: Uuid::new_v4(),
        expected_base: None,
        expected_new_head: head.to_owned(),
        branch: branch.to_owned(),
        object_format: format,
        snapshot_name: "raw-git-1-a-b-0".to_owned(),
    }
}

fn summary(head: &str, branch: &str, format: GitObjectFormat) -> GitVerifyResult {
    GitVerifyResult {
        head: head.to_owned(),
        branch: branch.to_owned(),
        base: None,
        object_format: format,
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
        .block_on(future)
}

struct FakeArtifact {
    artifact: VerifiedGitRepository,
    request: GitVerifyRequest,
    operation_root: PathBuf,
    paths: HelperPaths,
}

fn fake_artifact(parent: &Path) -> FakeArtifact {
    fake_artifact_for(parent, &"a".repeat(40), "main", GitObjectFormat::Sha1)
}

fn fake_artifact_for(
    parent: &Path,
    head: &str,
    branch: &str,
    format: GitObjectFormat,
) -> FakeArtifact {
    let request = request(head, branch, format);
    let operation_root = parent.join("git-verify-fake");
    fs::create_dir_all(operation_root.join("repo/objects/info")).expect("fake repo");
    fs::create_dir_all(operation_root.join("repo/refs/heads")).expect("fake refs");
    fs::write(
        operation_root.join("repo/HEAD"),
        format!("ref: refs/heads/{branch}\n"),
    )
    .expect("fake HEAD");
    fs::write(
        operation_root.join("repo/config"),
        "[core]\n\trepositoryformatversion = 0\n\tbare = true\n",
    )
    .expect("fake config");
    fs::create_dir_all(operation_root.join("home")).expect("fake home");
    fs::create_dir_all(operation_root.join("template")).expect("fake template");
    fs::set_permissions(&operation_root, fs::Permissions::from_mode(0o700))
        .expect("operation mode");
    let metadata = fs::metadata(&operation_root).expect("operation metadata");
    let paths = HelperPaths {
        operation_root: operation_root.clone(),
        repo_dir: operation_root.join("repo"),
        home_dir: operation_root.join("home"),
        template_dir: operation_root.join("template"),
        uid: metadata.uid(),
        gid: metadata.gid(),
    };
    let identity = VerifiedGitIdentity::from_validated_request(&request).expect("identity");
    let artifact = VerifiedGitRepository::new(
        open_dir(parent),
        "git-verify-fake".to_owned(),
        open_dir(&operation_root.join("repo")),
        summary(head, branch, format),
        identity,
        paths.clone(),
    );
    FakeArtifact {
        artifact,
        request,
        operation_root,
        paths,
    }
}

fn output(exit_code: i32, stdout: Vec<u8>, stderr: Vec<u8>) -> TrustedGitOutput {
    TrustedGitOutput {
        exit_code,
        stdout,
        stderr,
    }
}

struct RecordingRunner {
    results: VecDeque<Result<TrustedGitOutput, TrustedGitRunnerError>>,
    seen: Vec<TrustedGitInvocation>,
}

impl RecordingRunner {
    fn new(
        results: impl IntoIterator<Item = Result<TrustedGitOutput, TrustedGitRunnerError>>,
    ) -> Self {
        Self {
            results: results.into_iter().collect(),
            seen: Vec::new(),
        }
    }
}

impl TrustedGitRunner for RecordingRunner {
    fn run<'a>(&'a mut self, invocation: TrustedGitInvocation) -> TrustedGitFuture<'a> {
        Box::pin(async move {
            self.seen.push(invocation);
            self.results
                .pop_front()
                .unwrap_or_else(|| Ok(output(0, Vec::new(), Vec::new())))
        })
    }
}

struct PendingRunner;

impl TrustedGitRunner for PendingRunner {
    fn run<'a>(&'a mut self, _invocation: TrustedGitInvocation) -> TrustedGitFuture<'a> {
        Box::pin(std::future::pending())
    }
}

#[test]
fn export_limit_requires_positive_and_bounded_values() {
    assert_eq!(
        GitExportLimit::new(0),
        Err(GitStageError::InvalidExportLimit)
    );
    assert_eq!(
        GitExportLimit::new(MAX_EXPORT_BUNDLE_BYTES + 1),
        Err(GitStageError::ExportLimitTooLarge)
    );
    assert_eq!(
        GitExportLimit::new(MAX_EXPORT_BUNDLE_BYTES)
            .expect("ceiling")
            .max_bundle_bytes(),
        MAX_EXPORT_BUNDLE_BYTES
    );
}

#[test]
fn identity_and_summary_mismatch_are_rejected_before_runner() {
    let root = tempdir().expect("tempdir");
    let staging_parent = tempdir().expect("staging parent");
    let fake = fake_artifact(root.path());
    let mut changed = fake.request.clone();
    changed.authorization_generation = Uuid::new_v4();
    let mut runner = RecordingRunner::new([]);
    let error = block_on(stage_verified_bundle(
        fake.artifact,
        &changed,
        &open_dir(staging_parent.path()),
        &limits(),
        GitExportLimit::new(1024).expect("limit"),
        &mut runner,
    ))
    .expect_err("identity mismatch");
    assert_eq!(error, GitStageError::IdentityMismatch);
    assert!(runner.seen.is_empty());
    assert!(!fake.operation_root.exists());

    let fake = fake_artifact(root.path());
    let mut changed = fake.request.clone();
    changed.branch = "other".to_owned();
    let mut runner = RecordingRunner::new([]);
    let error = block_on(stage_verified_bundle(
        fake.artifact,
        &changed,
        &open_dir(staging_parent.path()),
        &limits(),
        GitExportLimit::new(1024).expect("limit"),
        &mut runner,
    ))
    .expect_err("identity mismatch");
    assert_eq!(error, GitStageError::IdentityMismatch);
    assert!(runner.seen.is_empty());
    assert!(!fake.operation_root.exists());
}

#[test]
fn alternates_are_rejected_before_runner() {
    for name in ["alternates", "http-alternates"] {
        let root = tempdir().expect("tempdir");
        let staging_parent = tempdir().expect("staging parent");
        let fake = fake_artifact(root.path());
        fs::write(
            fake.operation_root.join("repo/objects/info").join(name),
            b"../outside",
        )
        .expect("alternate");
        let mut runner = RecordingRunner::new([]);
        let error = block_on(stage_verified_bundle(
            fake.artifact,
            &fake.request,
            &open_dir(staging_parent.path()),
            &limits(),
            GitExportLimit::new(1024).expect("limit"),
            &mut runner,
        ))
        .expect_err("alternates");
        assert_eq!(error, GitStageError::AlternatesPresent);
        assert!(runner.seen.is_empty());
        assert!(!fake.operation_root.exists());
    }
}

#[test]
fn manager_ref_writer_handles_nested_and_sha256_refs_only() {
    for (format, head, branch) in [
        (GitObjectFormat::Sha1, "a".repeat(40), "feature/one"),
        (GitObjectFormat::Sha256, "b".repeat(64), "main"),
    ] {
        let root = tempdir().expect("tempdir");
        let canary = root.path().join("canary");
        fs::write(&canary, b"keep").expect("canary");
        let fake = fake_artifact_for(root.path(), &head, branch, format);
        let operation_root = fake.operation_root.clone();
        write_verified_ref(&fake.artifact, branch, &head).expect("ref");
        let ref_path = operation_root.join("repo/refs/heads").join(branch);
        assert_eq!(
            fs::read_to_string(&ref_path).expect("ref content"),
            format!("{head}\n")
        );
        let stat = fs::metadata(&ref_path).expect("ref stat");
        assert_eq!(stat.permissions().mode() & 0o777, 0o600);
        assert_eq!(stat.uid(), fake.paths.uid);
        assert_eq!(stat.nlink(), 1);
        drop(fake.artifact);
        assert!(!operation_root.exists());
        assert_eq!(fs::read(&canary).expect("canary"), b"keep");
    }
}

#[test]
fn manager_ref_writer_rejects_nonfresh_or_unproven_ref_layouts() {
    let root = tempdir().expect("tempdir");
    let outside = root.path().join("outside");
    fs::write(&outside, b"outside").expect("outside");

    for mutate in [
        |repo: &Path| fs::write(repo.join("refs/heads/main"), b"old\n").expect("existing ref"),
        |repo: &Path| fs::write(repo.join("packed-refs"), b"packed").expect("packed"),
        |repo: &Path| fs::create_dir(repo.join("reftable")).expect("reftable"),
        |repo: &Path| {
            let config = fs::read_to_string(repo.join("config")).expect("config");
            fs::write(
                repo.join("config"),
                format!("{config}[extensions]\n\trefStorage = reftable\n"),
            )
            .expect("config");
        },
        |repo: &Path| fs::write(repo.join("HEAD"), b"ref: refs/heads/other\n").expect("HEAD"),
        |repo: &Path| {
            let outside = repo.parent().expect("repo parent").join("outside");
            symlink(outside, repo.join("refs/heads/feature")).expect("symlink refs");
        },
    ] {
        let fake = fake_artifact_for(
            root.path(),
            &"a".repeat(40),
            "feature/one",
            GitObjectFormat::Sha1,
        );
        let operation_root = fake.operation_root.clone();
        mutate(&operation_root.join("repo"));
        let error = write_verified_ref(&fake.artifact, "feature/one", &"a".repeat(40))
            .expect_err("unsupported ref layout");
        assert!(matches!(
            error,
            GitStageError::RefExists | GitStageError::RefFormatUnsupported
        ));
        drop(fake.artifact);
        assert!(!operation_root.exists());
        assert_eq!(fs::read(&outside).expect("outside"), b"outside");
    }
}

#[test]
fn manager_ref_writer_rejects_malformed_branch_and_head() {
    let root = tempdir().expect("tempdir");
    let fake = fake_artifact(root.path());
    let operation_root = fake.operation_root.clone();
    assert_eq!(
        write_verified_ref(&fake.artifact, "main..x", &"a".repeat(40)),
        Err(GitStageError::RefWriteFailed)
    );
    assert_eq!(
        write_verified_ref(&fake.artifact, "main", &"A".repeat(40)),
        Err(GitStageError::RefWriteFailed)
    );
    drop(fake.artifact);
    assert!(!operation_root.exists());
}

#[test]
fn exact_output_limit_is_accepted_and_plus_one_is_rejected() {
    let head = "a".repeat(40);
    let bundle_limit = 64;
    let limit = bundle_limit + head.len() + 1;
    let root = tempdir().expect("tempdir");
    let staging_parent = tempdir().expect("staging parent");
    let canary = staging_parent.path().join("canary");
    fs::write(&canary, b"keep").expect("canary");
    let fake = fake_artifact(root.path());
    let paths = fake.paths.clone();
    let operation_root = fake.operation_root.clone();
    let bytes = vec![0xff_u8; bundle_limit];
    let mut runner = RecordingRunner::new([
        Ok(output(0, format!("{head}\n").into_bytes(), Vec::new())),
        Ok(output(0, bytes.clone(), Vec::new())),
    ]);
    let staged = block_on(stage_verified_bundle(
        fake.artifact,
        &fake.request,
        &open_dir(staging_parent.path()),
        &limits(),
        GitExportLimit::new(limit).expect("limit"),
        &mut runner,
    ))
    .expect("staged");
    assert_eq!(runner.seen.len(), 2);
    assert_ne!(
        runner.seen[0].plan.container_name(),
        runner.seen[1].plan.container_name()
    );
    assert_eq!(
        runner.seen[0].plan.max_output_bytes(),
        limit.min(limits().max_output_bytes)
    );
    assert_eq!(runner.seen[1].plan.max_output_bytes(), bundle_limit);
    let mount = format!(
        "type=bind,source={},target=/git-verify,ro",
        paths.operation_root.display()
    );
    assert!(runner.seen.iter().all(|invocation| {
        invocation
            .plan
            .docker_args()
            .iter()
            .any(|argument| argument == &mount)
    }));
    let staging_root = staging_parent.path().join(staged.staging_name());
    let bundle_path = staging_root.join("bundle.gitbundle");
    assert_eq!(fs::read(&bundle_path).expect("bundle"), bytes);
    let stat = rustix::fs::fstat(staged.bundle_fd()).expect("bundle stat");
    assert_eq!(stat.st_size, bundle_limit as i64);
    assert_eq!(stat.st_uid, paths.uid);
    assert_eq!(stat.st_nlink, 1);
    assert_eq!(stat.st_mode & 0o7777, 0o600);
    assert_eq!(staged.summary().head, fake.request.expected_new_head);
    assert!(staged.identity().matches(&fake.request));
    drop(staged);
    assert!(!operation_root.exists());
    assert!(!staging_root.exists());
    assert_eq!(fs::read(&canary).expect("canary"), b"keep");

    let fake = fake_artifact(root.path());
    let operation_root = fake.operation_root.clone();
    let mut runner = RecordingRunner::new([
        Ok(output(0, format!("{head}\n").into_bytes(), Vec::new())),
        Ok(output(0, vec![b'x'; bundle_limit + 1], Vec::new())),
    ]);
    let error = block_on(stage_verified_bundle(
        fake.artifact,
        &fake.request,
        &open_dir(staging_parent.path()),
        &limits(),
        GitExportLimit::new(limit).expect("limit"),
        &mut runner,
    ))
    .expect_err("over limit");
    assert_eq!(error, GitStageError::OutputLimit);
    assert!(!operation_root.exists());
    let names: Vec<_> = fs::read_dir(staging_parent.path())
        .expect("staging parent")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("canary")]);
}

#[test]
fn stderr_and_earlier_output_consume_the_shared_budget() {
    let head = "a".repeat(40);
    let stderr = 32;
    let limit = head.len() + 1 + stderr;
    let root = tempdir().expect("tempdir");
    let staging_parent = tempdir().expect("staging parent");
    let fake = fake_artifact(root.path());
    let operation_root = fake.operation_root.clone();
    let mut runner = RecordingRunner::new([
        Ok(output(
            0,
            format!("{head}\n").into_bytes(),
            vec![b'e'; stderr],
        )),
        Ok(output(0, vec![b'x'; 1], Vec::new())),
    ]);
    let error = block_on(stage_verified_bundle(
        fake.artifact,
        &fake.request,
        &open_dir(staging_parent.path()),
        &limits(),
        GitExportLimit::new(limit).expect("limit"),
        &mut runner,
    ))
    .expect_err("stderr consumed budget");
    assert_eq!(error, GitStageError::OutputLimit);
    assert_eq!(runner.seen.len(), 1);
    assert!(!operation_root.exists());
}

#[test]
fn show_ref_mismatch_is_rejected_before_bundle() {
    for stdout in [vec![b'b'; 40], b"not-hex".to_vec()] {
        let root = tempdir().expect("tempdir");
        let staging_parent = tempdir().expect("staging parent");
        let fake = fake_artifact(root.path());
        let operation_root = fake.operation_root.clone();
        let mut runner = RecordingRunner::new([Ok(output(0, stdout, Vec::new()))]);
        assert_eq!(
            block_on(stage_verified_bundle(
                fake.artifact,
                &fake.request,
                &open_dir(staging_parent.path()),
                &limits(),
                GitExportLimit::new(1024).expect("limit"),
                &mut runner,
            ))
            .expect_err("show-ref mismatch"),
            GitStageError::RefVerifyFailed
        );
        assert_eq!(runner.seen.len(), 1);
        assert!(!operation_root.exists());
    }
}

#[test]
fn cleanup_uncertainty_and_reap_timeout_retain_the_operation() {
    for (runner_error, expected) in [
        (
            TrustedGitRunnerError::Cleanup,
            GitStageError::CleanupUnconfirmed,
        ),
        (
            TrustedGitRunnerError::ReapTimeout,
            GitStageError::ReapTimeout,
        ),
    ] {
        let root = tempdir().expect("tempdir");
        let staging_parent = tempdir().expect("staging parent");
        let fake = fake_artifact(root.path());
        let operation_root = fake.operation_root.clone();
        let head = "a".repeat(40);
        let mut runner = RecordingRunner::new([
            Ok(output(0, format!("{head}\n").into_bytes(), Vec::new())),
            Err(runner_error),
        ]);
        let error = block_on(stage_verified_bundle(
            fake.artifact,
            &fake.request,
            &open_dir(staging_parent.path()),
            &limits(),
            GitExportLimit::new(1024).expect("limit"),
            &mut runner,
        ))
        .expect_err("uncertain cleanup");
        assert_eq!(error, expected);
        assert!(operation_root.join("repo").is_dir());
        fs::remove_dir_all(&operation_root).expect("test cleanup retained operation");
    }
}

#[test]
fn confirmed_failures_remove_the_operation() {
    for runner_error in [
        TrustedGitRunnerError::TimedOut,
        TrustedGitRunnerError::Cancelled,
        TrustedGitRunnerError::OutputLimit,
        TrustedGitRunnerError::Failed,
    ] {
        let root = tempdir().expect("tempdir");
        let staging_parent = tempdir().expect("staging parent");
        let fake = fake_artifact(root.path());
        let operation_root = fake.operation_root.clone();
        let mut runner = RecordingRunner::new([Err(runner_error)]);
        assert!(
            block_on(stage_verified_bundle(
                fake.artifact,
                &fake.request,
                &open_dir(staging_parent.path()),
                &limits(),
                GitExportLimit::new(1024).expect("limit"),
                &mut runner,
            ))
            .is_err()
        );
        assert!(!operation_root.exists());
    }
}

#[test]
fn aborting_before_runner_completion_retains_the_operation() {
    let root = tempdir().expect("tempdir");
    let staging_parent = tempdir().expect("staging parent");
    let fake = fake_artifact(root.path());
    let operation_root = fake.operation_root.clone();
    let mut runner = PendingRunner;
    let result = block_on(async {
        tokio::time::timeout(
            Duration::from_millis(50),
            stage_verified_bundle(
                fake.artifact,
                &fake.request,
                &open_dir(staging_parent.path()),
                &limits(),
                GitExportLimit::new(1024).expect("limit"),
                &mut runner,
            ),
        )
        .await
    });
    assert!(result.is_err(), "pending helper should time out");
    assert!(operation_root.join("repo").is_dir());
    fs::remove_dir_all(&operation_root).expect("test cleanup retained operation");
}

#[test]
fn repeated_cleanup_uncertainty_does_not_leak_fds() {
    let root = tempdir().expect("tempdir");
    let staging_parent = tempdir().expect("staging parent");
    let before = fs::read_dir("/proc/self/fd").expect("fd directory").count();
    for _ in 0..3 {
        let fake = fake_artifact(root.path());
        let operation_root = fake.operation_root.clone();
        let head = "a".repeat(40);
        let mut runner = RecordingRunner::new([
            Ok(output(0, format!("{head}\n").into_bytes(), Vec::new())),
            Err(TrustedGitRunnerError::Cleanup),
        ]);
        let error = block_on(stage_verified_bundle(
            fake.artifact,
            &fake.request,
            &open_dir(staging_parent.path()),
            &limits(),
            GitExportLimit::new(1024).expect("limit"),
            &mut runner,
        ))
        .expect_err("cleanup unconfirmed");
        assert_eq!(error, GitStageError::CleanupUnconfirmed);
        fs::remove_dir_all(&operation_root).expect("test cleanup retained operation");
    }
    let after = fs::read_dir("/proc/self/fd").expect("fd directory").count();
    assert!(
        after <= before + 1,
        "fd count grew from {before} to {after}"
    );
}

#[test]
fn benign_sha1_and_sha256_bundles_import_into_fresh_repositories() {
    for format in ["sha1", "sha256"] {
        let repo = RealRepo::new(format);
        repo.commit_file("file.txt", "hello\n", "initial");
        let head = repo.commit_file("file.txt", "second\n", "second");
        let raw = repo.stage();
        let request = repo.request(&raw, object_format(format));
        let quarantine = open_dir(&repo.quarantine);
        let artifact = block_on(verify_raw_git_snapshot(
            &quarantine,
            &raw,
            &request,
            &limits(),
            &mut HostRunner,
        ))
        .expect("verified artifact");
        let operation_root = artifact.operation_root().to_path_buf();
        let staging_parent = tempdir().expect("staging parent");
        let canary = staging_parent.path().join("canary");
        fs::write(&canary, b"keep").expect("canary");
        let staged = block_on(stage_verified_bundle(
            artifact,
            &request,
            &open_dir(staging_parent.path()),
            &limits(),
            GitExportLimit::new(MAX_EXPORT_BUNDLE_BYTES).expect("limit"),
            &mut HostRunner,
        ))
        .expect("staged");
        let staging_root = staging_parent.path().join(staged.staging_name());
        let bundle = staging_root.join("bundle.gitbundle");

        let empty = repo.temp.path().join("empty");
        fs::create_dir(&empty).expect("empty dir");
        git_init(&empty, format);
        let heads = git(
            &empty,
            &[
                "bundle",
                "list-heads",
                bundle.to_str().expect("bundle path"),
            ],
        );
        let lines: Vec<_> = heads.lines().collect();
        assert_eq!(lines.len(), 1, "one named ref");
        assert!(lines[0].ends_with(&format!("refs/heads/{}", repo.branch())));
        assert!(lines[0].starts_with(&head));
        git(
            &empty,
            &["bundle", "verify", bundle.to_str().expect("bundle path")],
        );

        let fresh = repo.temp.path().join("fresh");
        git_clone(&bundle, &fresh, &repo.branch());
        assert_eq!(git(&fresh, &["rev-parse", "HEAD"]), head);
        assert_eq!(
            git(&fresh, &["rev-list", "--all"]),
            git(&repo.repo, &["rev-list", "--all"])
        );

        drop(staged);
        assert!(!operation_root.exists());
        assert!(!staging_root.exists());
        assert_eq!(fs::read(&canary).expect("canary"), b"keep");
        assert!(repo.quarantine.join(&raw.snapshot_name).exists());
    }
}

fn object_format(format: &str) -> GitObjectFormat {
    match format {
        "sha1" => GitObjectFormat::Sha1,
        "sha256" => GitObjectFormat::Sha256,
        _ => panic!("unknown format"),
    }
}

struct RealRepo {
    temp: TempDir,
    repo: PathBuf,
    quarantine: PathBuf,
}

impl RealRepo {
    fn new(format: &str) -> Self {
        let temp = tempdir().expect("tempdir");
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

    fn request(&self, raw: &RawGitSnapshot, format: GitObjectFormat) -> GitVerifyRequest {
        GitVerifyRequest {
            owner: owner(11),
            workspace_binding_id: 11,
            code_project_id: Uuid::new_v4(),
            authorization_generation: Uuid::new_v4(),
            expected_base: None,
            expected_new_head: raw.captured_head.clone(),
            branch: raw.pinned_branch.clone(),
            object_format: format,
            snapshot_name: raw.snapshot_name.clone(),
        }
    }
}

struct HostRunner;

impl TrustedGitRunner for HostRunner {
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
    process.args(command_for(paths, command));
    process
}

fn command_for(paths: &HelperPaths, command: &TrustedGitCommand) -> Vec<String> {
    let repo = paths.repo_dir.to_string_lossy().into_owned();
    let template = paths.template_dir.to_string_lossy().into_owned();
    match command {
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

#[test]
#[ignore = "requires DF_GIT_HELPER_TEST_ALLOW=1, a digest-pinned helper image, a dedicated test daemon, and the operator isolation marker"]
fn ignored_real_container_bundle_helper() {
    let (image, host) = docker_test_guard();
    let repo = RealRepo::new("sha1");
    let head = repo.commit_file("file.txt", "hello\n", "initial");
    let raw = repo.stage();
    let request = repo.request(&raw, GitObjectFormat::Sha1);
    let quarantine = open_dir(&repo.quarantine);
    let mut docker_limits = limits();
    docker_limits.image = image.clone();
    let executor = DockerCliExecutor::new("docker".to_owned(), Some(host.clone()))
        .expect("dedicated docker executor");
    let mut runner = DockerTrustedGitRunner::new(executor, "docker".to_owned(), Some(host), None)
        .expect("docker runner");
    let artifact = block_on(verify_raw_git_snapshot(
        &quarantine,
        &raw,
        &request,
        &docker_limits,
        &mut runner,
    ))
    .expect("real container verification");
    let staging_parent = tempdir().expect("staging parent");
    let staged = block_on(stage_verified_bundle(
        artifact,
        &request,
        &open_dir(staging_parent.path()),
        &docker_limits,
        GitExportLimit::new(MAX_EXPORT_BUNDLE_BYTES).expect("limit"),
        &mut runner,
    ))
    .expect("real container bundle");
    let bundle = staging_parent
        .path()
        .join(staged.staging_name())
        .join("bundle.gitbundle");
    let empty = repo.temp.path().join("real-empty");
    fs::create_dir(&empty).expect("empty");
    git_init(&empty, "sha1");
    let heads = git(
        &empty,
        &["bundle", "list-heads", bundle.to_str().expect("bundle")],
    );
    assert_eq!(heads.lines().count(), 1);
    assert!(heads.contains(&head));
}

fn git_clone(bundle: &Path, fresh: &Path, branch: &str) {
    let output = base_git_command()
        .arg("clone")
        .arg("-b")
        .arg(branch)
        .arg(bundle)
        .arg(fresh)
        .output()
        .expect("git clone");
    assert!(
        output.status.success(),
        "git clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
