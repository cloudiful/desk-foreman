use super::clone_workspace::{CloneWorkspace, MOUNT_PROOF_CONTENT};
use crate::runtime::git_authority::clone_plan::{GitCloneConfig, GitCloneLimits};
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use uuid::Uuid;

fn config(root: &std::path::Path) -> GitCloneConfig {
    GitCloneConfig {
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
    }
}

#[test]
fn clone_workspace_proof_is_private_and_cleanup_is_scoped() {
    let root = tempfile::tempdir().expect("root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("root mode");
    let workspace = CloneWorkspace::create(&config(root.path()), Uuid::new_v4(), Uuid::new_v4())
        .expect("workspace");
    let operation = workspace.operation_dir().to_path_buf();
    assert_eq!(
        std::fs::read(workspace.control_dir().join(workspace.proof_name())).expect("proof"),
        MOUNT_PROOF_CONTENT
    );
    assert_eq!(
        std::fs::metadata(workspace.credential_dir())
            .expect("credential")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    workspace.cleanup().expect("cleanup");
    assert!(!operation.exists());
}
