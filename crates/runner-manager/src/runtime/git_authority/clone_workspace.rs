//! Private operation workspace for one isolated clone preparation.
//!
//! The operation root, control directory, credential directory and proof file
//! are created and pinned through manager-owned directory descriptors. The
//! model workspace is never mounted and cleanup removes only this operation's
//! own tree.

use std::fs::File;
use std::io::Write;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

use rustix::fs::{
    AtFlags, Dir, FileType, Mode, OFlags, fchmod, fstat, mkdirat, openat, statat, unlinkat,
};
use uuid::Uuid;

use super::clone_plan::GitCloneConfig;

pub(crate) const MOUNT_PROOF_CONTENT: &[u8] = b"desk-foreman git mount proof v1\n";
pub(crate) const MOUNT_PROOF_BLOB_SHA1: &str = "62eee4ed5398a1a8f6555a6ede7e20d7634a571e";
const PRIVATE_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;
const DIRECTORY_OPEN_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CloneWorkspaceError {
    InvalidRoot,
    CreateFailed,
    PathEscape,
    ProofFailed,
    CleanupFailed,
}

impl std::fmt::Display for CloneWorkspaceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRoot => "git clone private root is invalid",
            Self::CreateFailed => "git clone operation workspace could not be created",
            Self::PathEscape => "git clone operation workspace escaped its root",
            Self::ProofFailed => "git clone mount proof file failed",
            Self::CleanupFailed => "git clone workspace cleanup failed",
        })
    }
}

impl std::error::Error for CloneWorkspaceError {}

#[derive(Debug)]
pub(crate) struct CloneWorkspace {
    root_fd: OwnedFd,
    operation_fd: OwnedFd,
    control_fd: OwnedFd,
    credential_fd: OwnedFd,
    relative: String,
    operation_dir: PathBuf,
    control_dir: PathBuf,
    credential_dir: PathBuf,
    host_operation_dir: PathBuf,
    host_control_dir: PathBuf,
    host_credential_dir: PathBuf,
    proof_name: String,
    identity: WorkspaceIdentity,
    retained: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WorkspaceIdentity {
    pub(crate) dev: u64,
    pub(crate) ino: u64,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
}

impl CloneWorkspace {
    pub(crate) fn create(
        config: &GitCloneConfig,
        job_id: Uuid,
        operation_id: Uuid,
    ) -> Result<Self, CloneWorkspaceError> {
        let root_fd = rustix::fs::open(&config.private_root, DIRECTORY_OPEN_FLAGS, Mode::empty())
            .map_err(|_| CloneWorkspaceError::InvalidRoot)?;
        let root_stat = fstat(&root_fd).map_err(|_| CloneWorkspaceError::InvalidRoot)?;
        if FileType::from_raw_mode(root_stat.st_mode) != FileType::Directory
            || root_stat.st_uid != config.limits.uid
            || root_stat.st_mode & 0o7777 != PRIVATE_MODE
        {
            return Err(CloneWorkspaceError::InvalidRoot);
        }
        let relative = format!(
            "dfgit-op-{}-{}",
            job_id.simple(),
            &operation_id.simple().to_string()[..12]
        );
        let mode = Mode::from_bits_truncate(PRIVATE_MODE);
        mkdirat(&root_fd, relative.as_str(), mode)
            .map_err(|_| CloneWorkspaceError::CreateFailed)?;
        let operation_fd = openat(
            &root_fd,
            relative.as_str(),
            DIRECTORY_OPEN_FLAGS,
            Mode::empty(),
        )
        .map_err(|_| CloneWorkspaceError::CreateFailed)?;
        fchmod(&operation_fd, mode).map_err(|_| CloneWorkspaceError::CreateFailed)?;
        for name in ["control", "credential"] {
            mkdirat(&operation_fd, name, mode).map_err(|_| CloneWorkspaceError::CreateFailed)?;
        }
        let control_fd = openat(
            &operation_fd,
            "control",
            DIRECTORY_OPEN_FLAGS,
            Mode::empty(),
        )
        .map_err(|_| CloneWorkspaceError::CreateFailed)?;
        let credential_fd = openat(
            &operation_fd,
            "credential",
            DIRECTORY_OPEN_FLAGS,
            Mode::empty(),
        )
        .map_err(|_| CloneWorkspaceError::CreateFailed)?;
        fchmod(&control_fd, mode).map_err(|_| CloneWorkspaceError::CreateFailed)?;
        fchmod(&credential_fd, mode).map_err(|_| CloneWorkspaceError::CreateFailed)?;
        let operation_stat = fstat(&operation_fd).map_err(|_| CloneWorkspaceError::CreateFailed)?;
        if FileType::from_raw_mode(operation_stat.st_mode) != FileType::Directory {
            return Err(CloneWorkspaceError::PathEscape);
        }
        let operation_dir = config.private_root.join(&relative);
        let host_operation_dir = config.host_private_root.join(&relative);
        if !host_operation_dir.starts_with(&config.host_private_root) {
            return Err(CloneWorkspaceError::PathEscape);
        }
        let proof_name = format!("mount-proof-{}", Uuid::new_v4().simple());
        let file = openat(
            &control_fd,
            proof_name.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(PRIVATE_FILE_MODE),
        )
        .map_err(|_| CloneWorkspaceError::ProofFailed)?;
        let mut file = File::from(file);
        file.write_all(MOUNT_PROOF_CONTENT)
            .map_err(|_| CloneWorkspaceError::ProofFailed)?;
        file.sync_all()
            .map_err(|_| CloneWorkspaceError::ProofFailed)?;
        let identity = WorkspaceIdentity {
            dev: operation_stat.st_dev,
            ino: operation_stat.st_ino,
            uid: operation_stat.st_uid,
            gid: operation_stat.st_gid,
        };
        Ok(Self {
            root_fd,
            operation_fd,
            control_fd,
            credential_fd,
            relative,
            operation_dir: operation_dir.clone(),
            control_dir: operation_dir.join("control"),
            credential_dir: operation_dir.join("credential"),
            host_operation_dir: host_operation_dir.clone(),
            host_control_dir: host_operation_dir.join("control"),
            host_credential_dir: host_operation_dir.join("credential"),
            proof_name,
            identity,
            retained: false,
        })
    }

    pub(crate) fn control_dir(&self) -> &Path {
        &self.control_dir
    }

    pub(crate) fn credential_dir(&self) -> &Path {
        &self.credential_dir
    }

    pub(crate) fn host_control_dir(&self) -> &Path {
        &self.host_control_dir
    }

    pub(crate) fn host_credential_dir(&self) -> &Path {
        &self.host_credential_dir
    }

    pub(crate) fn proof_name(&self) -> &str {
        &self.proof_name
    }

    pub(crate) fn identity(&self) -> WorkspaceIdentity {
        self.identity
    }

    pub(crate) fn operation_dir(&self) -> &Path {
        &self.operation_dir
    }

    pub(crate) fn host_operation_dir(&self) -> &Path {
        &self.host_operation_dir
    }

    /// Recheck the pinned descriptor and the root entry before later plans.
    pub(crate) fn revalidate(&self) -> Result<(), CloneWorkspaceError> {
        let operation = fstat(&self.operation_fd).map_err(|_| CloneWorkspaceError::PathEscape)?;
        if operation.st_dev != self.identity.dev
            || operation.st_ino != self.identity.ino
            || operation.st_uid != self.identity.uid
            || operation.st_gid != self.identity.gid
            || FileType::from_raw_mode(operation.st_mode) != FileType::Directory
        {
            return Err(CloneWorkspaceError::PathEscape);
        }
        let entry = statat(
            &self.root_fd,
            self.relative.as_str(),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(|_| CloneWorkspaceError::PathEscape)?;
        if entry.st_dev != self.identity.dev
            || entry.st_ino != self.identity.ino
            || entry.st_uid != self.identity.uid
            || entry.st_gid != self.identity.gid
            || FileType::from_raw_mode(entry.st_mode) != FileType::Directory
        {
            return Err(CloneWorkspaceError::PathEscape);
        }
        for fd in [&self.control_fd, &self.credential_fd] {
            let stat = fstat(fd).map_err(|_| CloneWorkspaceError::PathEscape)?;
            if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
                return Err(CloneWorkspaceError::PathEscape);
            }
        }
        Ok(())
    }

    /// Remove the asked-for credential files only after credentialed helpers
    /// have confirmed termination.
    pub(crate) fn remove_credentials(&self) -> Result<(), CloneWorkspaceError> {
        remove_tree_contents(&self.credential_fd);
        unlinkat(&self.operation_fd, "credential", AtFlags::REMOVEDIR)
            .map_err(|_| CloneWorkspaceError::CleanupFailed)
    }

    /// Hand ownership to reconciliation without deleting a possibly live tree.
    pub(crate) fn retain(mut self) -> PathBuf {
        self.retained = true;
        self.operation_dir.clone()
    }

    /// Best-effort cleanup of this operation only.
    pub(crate) fn cleanup(mut self) -> Result<(), CloneWorkspaceError> {
        self.retained = true;
        remove_tree_contents(&self.operation_fd);
        unlinkat(&self.root_fd, self.relative.as_str(), AtFlags::REMOVEDIR)
            .map_err(|_| CloneWorkspaceError::CleanupFailed)
    }
}

impl Drop for CloneWorkspace {
    fn drop(&mut self) {
        if !self.retained {
            remove_tree_contents(&self.operation_fd);
            let _ = unlinkat(&self.root_fd, self.relative.as_str(), AtFlags::REMOVEDIR);
        }
    }
}

fn remove_tree_contents(dir: &OwnedFd) {
    let mut reader = match Dir::read_from(dir) {
        Ok(reader) => reader,
        Err(_) => return,
    };
    while let Some(entry) = reader.read() {
        let Ok(entry) = entry else { break };
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        let is_directory = match entry.file_type() {
            FileType::Directory => true,
            FileType::Unknown => statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)
                .map(|stat| FileType::from_raw_mode(stat.st_mode) == FileType::Directory)
                .unwrap_or(false),
            _ => false,
        };
        if is_directory {
            if let Ok(child) = openat(dir, name, DIRECTORY_OPEN_FLAGS, Mode::empty()) {
                remove_tree_contents(&child);
            }
            let _ = unlinkat(dir, name, AtFlags::REMOVEDIR);
        } else {
            let _ = unlinkat(dir, name, AtFlags::empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::git_authority::clone_plan::GitCloneLimits;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    fn private_tempdir() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).expect("mode");
        temp
    }

    fn config(root: &Path, host: &Path) -> GitCloneConfig {
        GitCloneConfig {
            private_root: root.to_path_buf(),
            host_private_root: host.to_path_buf(),
            limits: GitCloneLimits {
                image: format!("helper@sha256:{}", "a".repeat(64)),
                network: "df-git-net".to_owned(),
                broker_host: "broker.example.invalid".to_owned(),
                uid: rustix::process::geteuid().as_raw(),
                gid: rustix::process::getegid().as_raw(),
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
        }
    }

    #[test]
    fn creates_owned_dirs_proof_and_host_mapping() {
        let private = private_tempdir();
        let host = tempfile::tempdir().expect("host");
        let workspace = CloneWorkspace::create(
            &config(private.path(), host.path()),
            Uuid::new_v4(),
            Uuid::new_v4(),
        )
        .expect("workspace");
        assert_eq!(
            fs::read(workspace.control_dir().join(workspace.proof_name())).expect("proof"),
            MOUNT_PROOF_CONTENT
        );
        assert_eq!(
            fs::metadata(workspace.control_dir())
                .expect("control")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert!(workspace.host_control_dir().starts_with(host.path()));
        assert!(workspace.host_credential_dir().starts_with(host.path()));
        assert!(workspace.control_dir().starts_with(private.path()));
        workspace.revalidate().expect("identity");
        let operation_dir = workspace.operation_dir().to_path_buf();
        workspace.cleanup().expect("cleanup");
        assert!(!operation_dir.exists());
    }

    #[test]
    fn cleanup_removes_only_its_own_operation() {
        let private = private_tempdir();
        let other = private.path().join("other");
        fs::create_dir(&other).expect("other");
        let workspace = CloneWorkspace::create(
            &config(private.path(), private.path()),
            Uuid::new_v4(),
            Uuid::new_v4(),
        )
        .expect("workspace");
        let operation_dir = workspace.operation_dir().to_path_buf();
        workspace.cleanup().expect("cleanup");
        assert!(other.exists());
        assert!(!operation_dir.exists());
    }
}
