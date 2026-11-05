use super::clone_bundle::{GitBundleError, GitBundleIdentity, VerifiedGitBundle};
use super::clone_fetch::{ObjectStats, TreeStats};
use crate::runtime::git_authority::clone_plan::{GitCloneConfig, GitCloneLimits};
use crate::runtime::git_authority::clone_workspace::CloneWorkspace;
use runner_protocol::GitObjectFormat;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use uuid::Uuid;

fn workspace(root: &std::path::Path) -> CloneWorkspace {
    let config = GitCloneConfig {
        private_root: root.to_path_buf(),
        host_private_root: root.to_path_buf(),
        limits: GitCloneLimits {
            image: format!("helper@sha256:{}", "a".repeat(64)),
            network: "df-git-net".to_owned(),
            broker_host: "broker.example.invalid".to_owned(),
            uid: 1000,
            gid: 1000,
            fetch_volume_bytes: 1024,
            bundle_bytes: 1024,
            logical_object_bytes: 1024,
            checkout_bytes: 1024,
            tree_entries: 8,
            deadline: Duration::from_secs(10),
            memory_limit: "256m".to_owned(),
            cpu_limit: "1".to_owned(),
            pids_limit: 16,
        },
    };
    CloneWorkspace::create(&config, Uuid::new_v4(), Uuid::new_v4()).expect("workspace")
}

fn identity() -> GitBundleIdentity {
    GitBundleIdentity {
        job_id: Uuid::new_v4(),
        operation_id: Uuid::new_v4(),
        application_id: 1,
        workspace_binding_id: 2,
        principal_digest: "a".repeat(64),
        code_project_id: Uuid::new_v4(),
        authorization_generation: Uuid::new_v4(),
        branch: "main".to_owned(),
        expected_base: None,
        head: "b".repeat(40),
        object_format: GitObjectFormat::Sha1,
    }
}

#[test]
fn clone_bundle_rejects_oversized_output() {
    let root = tempfile::tempdir().expect("root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("root mode");
    let tree = TreeStats {
        entries: 0,
        total_bytes: 0,
        files: 0,
        symlinks: 0,
        gitlinks: 0,
    };
    let objects = ObjectStats {
        objects: 0,
        total_bytes: 0,
    };
    assert!(matches!(
        VerifiedGitBundle::store(
            workspace(root.path()),
            identity(),
            b"too-big",
            1,
            tree,
            objects
        ),
        Err(GitBundleError::TooLarge)
    ));
}
