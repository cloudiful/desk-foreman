use std::fs;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::symlink;
use std::path::Path;

use runner_protocol::{GitObjectFormat, GitVerifyRequest, GitVerifyResult, RunnerOwner};
use uuid::Uuid;

use super::artifact::{VerifiedGitIdentity, VerifiedGitRepository};
use super::helper::HelperPaths;

fn open_dir(path: &Path) -> OwnedFd {
    fs::File::open(path).expect("open dir").into()
}

fn summary() -> GitVerifyResult {
    GitVerifyResult {
        head: "0123456789abcdef0123456789abcdef01234567".to_owned(),
        branch: "main".to_owned(),
        base: None,
        object_format: GitObjectFormat::Sha1,
    }
}

fn request() -> GitVerifyRequest {
    GitVerifyRequest {
        owner: RunnerOwner::ApplicationPrincipal {
            application_id: 7,
            workspace_binding_id: 11,
            principal_digest: "principal-digest".to_owned(),
        },
        workspace_binding_id: 11,
        code_project_id: Uuid::from_u128(1),
        authorization_generation: Uuid::from_u128(2),
        expected_base: None,
        expected_new_head: summary().head,
        branch: "main".to_owned(),
        object_format: GitObjectFormat::Sha1,
        snapshot_name: "raw-git-1-a-b-0".to_owned(),
    }
}

fn identity() -> VerifiedGitIdentity {
    VerifiedGitIdentity::from_validated_request(&request()).expect("identity")
}

fn paths(root: &Path) -> HelperPaths {
    HelperPaths {
        operation_root: root.to_path_buf(),
        repo_dir: root.join("repo"),
        home_dir: root.join("home"),
        template_dir: root.join("template"),
        uid: 1000,
        gid: 1000,
    }
}

#[test]
fn drop_removes_only_the_owned_operation_directory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let parent = temp.path();
    let operation = parent.join("git-verify-1");
    fs::create_dir_all(operation.join("repo")).expect("operation");
    fs::write(operation.join("repo/marker"), b"verified").expect("repo marker");
    fs::write(parent.join("raw-git-1-a-b-0"), b"raw snapshot").expect("raw");
    fs::write(parent.join("outside-canary"), b"keep").expect("canary");
    symlink(parent.join("outside-canary"), parent.join("link-canary")).expect("symlink");

    let artifact = VerifiedGitRepository::new(
        open_dir(parent),
        "git-verify-1".to_owned(),
        open_dir(&operation.join("repo")),
        summary(),
        identity(),
        paths(&operation),
    );
    assert_eq!(artifact.summary().head, summary().head);
    assert_eq!(artifact.operation_root(), operation);
    assert_eq!(artifact.helper_paths().repo_dir, operation.join("repo"));
    assert!(artifact.identity().matches(&request()));
    assert!(artifact.is_quiescent());
    assert!(artifact.repo_fd().as_raw_fd() >= 0);
    let debug = format!("{artifact:?}");
    assert!(debug.contains("VerifiedGitRepository"));
    assert!(!debug.contains(parent.to_string_lossy().as_ref()));

    drop(artifact);
    assert!(!operation.exists());
    assert_eq!(
        fs::read(parent.join("raw-git-1-a-b-0")).expect("raw"),
        b"raw snapshot"
    );
    assert_eq!(
        fs::read(parent.join("outside-canary")).expect("canary"),
        b"keep"
    );
    assert!(
        fs::symlink_metadata(parent.join("link-canary"))
            .expect("link")
            .file_type()
            .is_symlink()
    );
}

#[test]
fn nonquiescent_artifact_retains_the_operation_but_closes_fds() {
    let temp = tempfile::tempdir().expect("tempdir");
    let parent = temp.path();
    let operation = parent.join("git-verify-retained");
    fs::create_dir_all(operation.join("repo")).expect("operation");
    let mut artifact = VerifiedGitRepository::new(
        open_dir(parent),
        "git-verify-retained".to_owned(),
        open_dir(&operation.join("repo")),
        summary(),
        identity(),
        paths(&operation),
    );
    artifact.begin_operation();
    assert!(!artifact.is_quiescent());
    drop(artifact);
    assert!(operation.join("repo").is_dir());
    fs::remove_dir_all(&operation).expect("test cleanup retained operation");
}

#[test]
fn confirmed_quiescence_restores_drop_cleanup() {
    let temp = tempfile::tempdir().expect("tempdir");
    let parent = temp.path();
    let operation = parent.join("git-verify-confirmed");
    fs::create_dir_all(operation.join("repo")).expect("operation");
    let mut artifact = VerifiedGitRepository::new(
        open_dir(parent),
        "git-verify-confirmed".to_owned(),
        open_dir(&operation.join("repo")),
        summary(),
        identity(),
        paths(&operation),
    );
    artifact.begin_operation();
    artifact.confirm_quiescent();
    assert!(artifact.is_quiescent());
    drop(artifact);
    assert!(!operation.exists());
}

#[test]
fn raw_snapshot_mutation_does_not_change_the_owned_artifact() {
    let temp = tempfile::tempdir().expect("tempdir");
    let parent = temp.path();
    let operation = parent.join("git-verify-2");
    fs::create_dir_all(operation.join("repo")).expect("operation");
    fs::write(operation.join("repo/marker"), b"verified").expect("repo marker");
    let raw = parent.join("raw-git-1-a-b-0");
    fs::create_dir_all(&raw).expect("raw");
    fs::write(raw.join("payload"), b"raw payload").expect("raw payload");

    let artifact = VerifiedGitRepository::new(
        open_dir(parent),
        "git-verify-2".to_owned(),
        open_dir(&operation.join("repo")),
        summary(),
        identity(),
        paths(&operation),
    );
    fs::remove_dir_all(&raw).expect("remove raw");
    assert_eq!(artifact.summary().head, summary().head);
    assert_eq!(artifact.summary().branch, "main");
    drop(artifact);
    assert!(!operation.exists());
}
