//! Owned, bounded bundle prepared from a verified fetched repository.
//!
//! The bundle is a plain binary file created on the host only after the helper
//! stream is bounded. It is not imported into a host Git repository and it does
//! not declare job success or reservation release.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

use runner_protocol::GitObjectFormat;
use uuid::Uuid;

use super::clone_fetch::{ObjectStats, TreeStats};
use super::clone_workspace::{CloneWorkspace, CloneWorkspaceError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GitBundleIdentity {
    pub(crate) job_id: Uuid,
    pub(crate) operation_id: Uuid,
    pub(crate) application_id: i64,
    pub(crate) workspace_binding_id: i64,
    pub(crate) principal_digest: String,
    pub(crate) code_project_id: Uuid,
    pub(crate) authorization_generation: Uuid,
    pub(crate) branch: String,
    pub(crate) expected_base: Option<String>,
    pub(crate) head: String,
    pub(crate) object_format: GitObjectFormat,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GitBundleStat {
    pub(crate) source_path: std::path::PathBuf,
    pub(crate) dev: u64,
    pub(crate) ino: u64,
    pub(crate) uid: u32,
    pub(crate) size: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GitBundleError {
    InvalidIdentity,
    TooLarge,
    FileFailed,
    CleanupFailed,
}

impl std::fmt::Display for GitBundleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidIdentity => "verified git bundle identity is invalid",
            Self::TooLarge => "verified git bundle exceeds its limit",
            Self::FileFailed => "verified git bundle file failed",
            Self::CleanupFailed => "verified git bundle cleanup failed",
        })
    }
}

impl std::error::Error for GitBundleError {}

/// Immutable verified result owned by C1. Not Clone, not Serialize, and no
/// public path accessor.
pub(crate) struct VerifiedGitBundle {
    identity: GitBundleIdentity,
    path: std::path::PathBuf,
    bundle_bytes: u64,
    tree: TreeStats,
    objects: ObjectStats,
    workspace: Option<CloneWorkspace>,
    bundle_dev: u64,
    bundle_ino: u64,
    bundle_uid: u32,
    quiescent: bool,
}

impl std::fmt::Debug for VerifiedGitBundle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("VerifiedGitBundle(..)")
    }
}

impl VerifiedGitBundle {
    pub(crate) fn store(
        workspace: CloneWorkspace,
        identity: GitBundleIdentity,
        bytes: &[u8],
        max_bundle_bytes: u64,
        tree: TreeStats,
        objects: ObjectStats,
    ) -> Result<Self, GitBundleError> {
        if identity.job_id.is_nil()
            || identity.operation_id.is_nil()
            || identity.application_id <= 0
            || identity.workspace_binding_id <= 0
            || identity.code_project_id.is_nil()
            || identity.authorization_generation.is_nil()
            || !(is_lower_hex(&identity.head, 40) || is_lower_hex(&identity.head, 64))
        {
            return Err(GitBundleError::InvalidIdentity);
        }
        if bytes.len() as u64 > max_bundle_bytes {
            return Err(GitBundleError::TooLarge);
        }
        let path = workspace.operation_dir().join("bundle.git");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|_| GitBundleError::FileFailed)?;
        file.write_all(bytes)
            .map_err(|_| GitBundleError::FileFailed)?;
        file.sync_all().map_err(|_| GitBundleError::FileFailed)?;
        let metadata = file.metadata().map_err(|_| GitBundleError::FileFailed)?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.len() != bytes.len() as u64
            || metadata.mode() & 0o077 != 0
        {
            return Err(GitBundleError::FileFailed);
        }
        Ok(Self {
            identity,
            path,
            bundle_bytes: bytes.len() as u64,
            tree,
            objects,
            workspace: Some(workspace),
            bundle_dev: metadata.dev(),
            bundle_ino: metadata.ino(),
            bundle_uid: metadata.uid(),
            quiescent: true,
        })
    }

    pub(crate) fn identity(&self) -> &GitBundleIdentity {
        &self.identity
    }

    pub(crate) fn bundle_bytes(&self) -> u64 {
        self.bundle_bytes
    }

    pub(crate) fn tree(&self) -> TreeStats {
        self.tree
    }

    pub(crate) fn objects(&self) -> ObjectStats {
        self.objects
    }

    /// Recheck the captured bundle identity before another helper.
    pub(crate) fn revalidate_mount(&self) -> Result<(), GitBundleError> {
        self.mount_stat().map(|_| ())
    }

    /// Mark the bundle non-quiescent synchronously before a helper await that
    /// may still access the private operation tree.
    pub(crate) fn mark_busy(&mut self) {
        self.quiescent = false;
    }

    /// Mark every helper using the bundle as terminated and cleaned up.
    pub(crate) fn mark_quiescent(&mut self) {
        self.quiescent = true;
    }

    pub(crate) fn is_quiescent(&self) -> bool {
        self.quiescent
    }

    /// Exact bundle mount source and captured immutable identity.
    pub(crate) fn mount_stat(&self) -> Result<GitBundleStat, GitBundleError> {
        let workspace = self.workspace.as_ref().ok_or(GitBundleError::FileFailed)?;
        let metadata =
            std::fs::symlink_metadata(&self.path).map_err(|_| GitBundleError::FileFailed)?;
        let identity = workspace.identity();
        if !metadata.file_type().is_file()
            || metadata.nlink() != 1
            || metadata.mode() & 0o077 != 0
            || metadata.uid() != self.bundle_uid
            || metadata.dev() != self.bundle_dev
            || metadata.ino() != self.bundle_ino
            || metadata.uid() != identity.uid
            || metadata.len() != self.bundle_bytes
        {
            return Err(GitBundleError::FileFailed);
        }
        Ok(GitBundleStat {
            source_path: workspace.host_operation_dir().join("bundle.git"),
            dev: metadata.dev(),
            ino: metadata.ino(),
            uid: metadata.uid(),
            size: metadata.len(),
        })
    }

    /// Explicit cleanup reports failures; a non-quiescent bundle is retained.
    pub(crate) fn cleanup(mut self) -> Result<(), GitBundleError> {
        if !self.quiescent {
            if let Some(workspace) = self.workspace.take() {
                let _ = workspace.retain();
            }
            return Err(GitBundleError::CleanupFailed);
        }
        if let Some(workspace) = self.workspace.take() {
            workspace.cleanup().map_err(|error| match error {
                CloneWorkspaceError::CleanupFailed => GitBundleError::CleanupFailed,
                _ => GitBundleError::CleanupFailed,
            })?;
        } else if self.path.exists() {
            std::fs::remove_file(&self.path).map_err(|_| GitBundleError::CleanupFailed)?;
        }
        Ok(())
    }
}

impl Drop for VerifiedGitBundle {
    fn drop(&mut self) {
        if let Some(workspace) = self.workspace.take() {
            if self.quiescent {
                let _ = workspace.cleanup();
            } else {
                let _ = workspace.retain();
            }
        }
    }
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::git_authority::clone_plan::{GitCloneConfig, GitCloneLimits};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::Duration;

    fn private_tempdir() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("temp");
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700))
            .expect("mode");
        temp
    }

    fn workspace(root: &Path) -> CloneWorkspace {
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
                tree_entries: 10,
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

    fn stats() -> (TreeStats, ObjectStats) {
        (
            TreeStats {
                entries: 1,
                total_bytes: 2,
                files: 1,
                symlinks: 0,
                gitlinks: 0,
            },
            ObjectStats {
                objects: 1,
                total_bytes: 2,
            },
        )
    }

    #[test]
    fn bundle_is_owned_regular_0600_and_bounded() {
        let temp = private_tempdir();
        let (tree, objects) = stats();
        let bundle = VerifiedGitBundle::store(
            workspace(temp.path()),
            identity(),
            b"bundle-bytes",
            1024,
            tree,
            objects,
        )
        .expect("bundle");
        assert_eq!(bundle.bundle_bytes(), 12);
        let metadata = std::fs::metadata(
            bundle
                .workspace
                .as_ref()
                .expect("workspace")
                .operation_dir()
                .join("bundle.git"),
        )
        .expect("metadata");
        assert_eq!(metadata.mode() & 0o777, 0o600);
        assert_eq!(metadata.nlink(), 1);
        bundle.cleanup().expect("cleanup");
    }

    #[test]
    fn oversized_or_invalid_bundle_is_rejected() {
        let temp = private_tempdir();
        let (tree, objects) = stats();
        assert!(matches!(
            VerifiedGitBundle::store(
                workspace(temp.path()),
                identity(),
                b"too-large",
                1,
                tree,
                objects
            ),
            Err(GitBundleError::TooLarge)
        ));
        let mut bad = identity();
        bad.head = "A".repeat(40);
        assert!(matches!(
            VerifiedGitBundle::store(workspace(temp.path()), bad, b"x", 1024, tree, objects),
            Err(GitBundleError::InvalidIdentity)
        ));
    }

    #[test]
    fn debug_is_redacted() {
        let temp = private_tempdir();
        let (tree, objects) = stats();
        let bundle = VerifiedGitBundle::store(
            workspace(temp.path()),
            identity(),
            b"bundle",
            1024,
            tree,
            objects,
        )
        .expect("bundle");
        assert_eq!(format!("{bundle:?}"), "VerifiedGitBundle(..)");
    }
}
