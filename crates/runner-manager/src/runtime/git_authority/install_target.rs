//! Exact resource-owned install target with fd-relative publication.
//!
//! The target is derived from the shared resource-workspace hash, pinned from
//! the manager workspace root by descriptor, and published with NOREPLACE into
//! an absent or empty directory only.

use std::ffi::OsStr;
use std::os::fd::OwnedFd;
use std::path::{Component, Path, PathBuf};
use tokio::time::Instant;

use rustix::fs::{
    AtFlags, Dir, FileType, Mode, OFlags, RenameFlags, fchmod, fstat, mkdirat, openat,
    renameat_with, statat, unlinkat,
};
use rustix::io::Errno;
use tokio::sync::watch;
use uuid::Uuid;

use super::clone_workspace::WorkspaceIdentity;

const DIRECTORY_OPEN_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const PRIVATE_MODE: u32 = 0o700;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallTargetError {
    InvalidPath,
    UnsafeParent,
    TargetNotEmpty,
    CreateFailed,
    IdentityChanged,
    PublishFailed,
    Timeout,
    Cancelled,
    CleanupFailed,
}

impl std::fmt::Display for InstallTargetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidPath => "install target path is invalid",
            Self::UnsafeParent => "install target parent is unsafe",
            Self::TargetNotEmpty => "install target is not empty",
            Self::CreateFailed => "install target could not be created",
            Self::IdentityChanged => "install target identity changed",
            Self::PublishFailed => "install target could not be published",
            Self::Timeout => "install target timed out",
            Self::Cancelled => "install target was cancelled",
            Self::CleanupFailed => "install target cleanup failed",
        })
    }
}

impl std::error::Error for InstallTargetError {}

#[derive(Debug)]
pub(crate) struct InstallTarget {
    resources_fd: OwnedFd,
    temp_fd: OwnedFd,
    target_name: String,
    temp_name: String,
    target_path: PathBuf,
    identity: WorkspaceIdentity,
    published: bool,
    retained: bool,
}

impl InstallTarget {
    pub(crate) fn open(
        workspace_root: &Path,
        application_id: i64,
        code_project_id: Uuid,
        requested_root: &str,
        uid: u32,
        gid: u32,
    ) -> Result<Self, InstallTargetError> {
        if application_id <= 0 {
            return Err(InstallTargetError::InvalidPath);
        }
        let target_name = desk_foreman::workspace::resource_workspace_hash(
            "code_project",
            &code_project_id.to_string(),
        );
        let expected = workspace_root
            .join("apps")
            .join(application_id.to_string())
            .join("resources")
            .join(&target_name);
        let requested = Path::new(requested_root);
        if !requested.is_absolute() || requested != expected {
            return Err(InstallTargetError::InvalidPath);
        }
        if requested
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
        {
            return Err(InstallTargetError::InvalidPath);
        }

        let mut directory = rustix::fs::open(workspace_root, DIRECTORY_OPEN_FLAGS, Mode::empty())
            .map_err(|_| InstallTargetError::UnsafeParent)?;
        for component in ["apps", &application_id.to_string(), "resources"] {
            check_parent(&directory, uid)?;
            let next = openat(&directory, component, DIRECTORY_OPEN_FLAGS, Mode::empty())
                .map_err(|_| InstallTargetError::UnsafeParent)?;
            let stat = fstat(&next).map_err(|_| InstallTargetError::UnsafeParent)?;
            if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
                || stat.st_uid != uid
                || stat.st_mode & 0o022 != 0
            {
                return Err(InstallTargetError::UnsafeParent);
            }
            directory = next;
        }
        check_parent(&directory, uid)?;

        match statat(&directory, target_name.as_str(), AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => {
                if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
                    || stat.st_uid != uid
                    || stat.st_mode & 0o077 != 0
                {
                    return Err(InstallTargetError::UnsafeParent);
                }
                let target_fd = openat(
                    &directory,
                    target_name.as_str(),
                    DIRECTORY_OPEN_FLAGS,
                    Mode::empty(),
                )
                .map_err(|_| InstallTargetError::TargetNotEmpty)?;
                if !is_empty_dir(&target_fd) {
                    return Err(InstallTargetError::TargetNotEmpty);
                }
            }
            Err(Errno::NOENT) => {}
            Err(_) => return Err(InstallTargetError::InvalidPath),
        }

        let temp_name = format!("dfgit-install-{}", Uuid::new_v4().simple());
        let mode = Mode::from_bits_truncate(PRIVATE_MODE);
        mkdirat(&directory, temp_name.as_str(), mode)
            .map_err(|_| InstallTargetError::CreateFailed)?;
        let temp_fd = openat(
            &directory,
            temp_name.as_str(),
            DIRECTORY_OPEN_FLAGS,
            Mode::empty(),
        )
        .map_err(|_| InstallTargetError::CreateFailed)?;
        fchmod(&temp_fd, mode).map_err(|_| InstallTargetError::CreateFailed)?;
        let stat = fstat(&temp_fd).map_err(|_| InstallTargetError::CreateFailed)?;
        if stat.st_gid != gid {
            return Err(InstallTargetError::CreateFailed);
        }
        let identity = WorkspaceIdentity {
            dev: stat.st_dev,
            ino: stat.st_ino,
            uid: stat.st_uid,
            gid: stat.st_gid,
        };
        Ok(Self {
            resources_fd: directory,
            temp_fd,
            target_name,
            temp_name,
            target_path: expected,
            identity,
            published: false,
            retained: false,
        })
    }

    pub(crate) fn target_path(&self) -> &Path {
        &self.target_path
    }

    pub(crate) fn temp_fd(&self) -> &OwnedFd {
        &self.temp_fd
    }

    pub(crate) fn publish(
        &mut self,
        cancel: &watch::Receiver<bool>,
        deadline: Instant,
    ) -> Result<(), InstallTargetError> {
        if *cancel.borrow() {
            return Err(InstallTargetError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(InstallTargetError::Timeout);
        }
        let stat = fstat(&self.temp_fd).map_err(|_| InstallTargetError::IdentityChanged)?;
        if stat.st_dev != self.identity.dev
            || stat.st_ino != self.identity.ino
            || stat.st_uid != self.identity.uid
            || stat.st_gid != self.identity.gid
            || FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        {
            return Err(InstallTargetError::IdentityChanged);
        }
        match statat(
            &self.resources_fd,
            self.target_name.as_str(),
            AtFlags::SYMLINK_NOFOLLOW,
        ) {
            Ok(target) => {
                if FileType::from_raw_mode(target.st_mode) != FileType::Directory
                    || target.st_uid != self.identity.uid
                    || target.st_mode & 0o077 != 0
                {
                    return Err(InstallTargetError::PublishFailed);
                }
                let target_fd = openat(
                    &self.resources_fd,
                    self.target_name.as_str(),
                    DIRECTORY_OPEN_FLAGS,
                    Mode::empty(),
                )
                .map_err(|_| InstallTargetError::PublishFailed)?;
                if !is_empty_dir(&target_fd) {
                    return Err(InstallTargetError::TargetNotEmpty);
                }
                drop(target_fd);
                unlinkat(
                    &self.resources_fd,
                    self.target_name.as_str(),
                    AtFlags::REMOVEDIR,
                )
                .map_err(|_| InstallTargetError::PublishFailed)?;
            }
            Err(Errno::NOENT) => {}
            Err(_) => return Err(InstallTargetError::PublishFailed),
        }
        renameat_with(
            &self.resources_fd,
            self.temp_name.as_str(),
            &self.resources_fd,
            self.target_name.as_str(),
            RenameFlags::NOREPLACE,
        )
        .map_err(|_| InstallTargetError::PublishFailed)?;
        self.published = true;
        Ok(())
    }

    pub(crate) fn published(&self) -> bool {
        self.published
    }

    /// Remove only the unpublished temporary directory.
    pub(crate) fn cleanup(mut self) -> Result<(), InstallTargetError> {
        self.retained = true;
        if self.published {
            return Ok(());
        }
        remove_tree_contents(&self.temp_fd);
        unlinkat(
            &self.resources_fd,
            self.temp_name.as_str(),
            AtFlags::REMOVEDIR,
        )
        .map_err(|_| InstallTargetError::CleanupFailed)
    }

    pub(crate) fn retain(mut self) {
        self.retained = true;
    }
}

impl Drop for InstallTarget {
    fn drop(&mut self) {
        if !self.retained && !self.published {
            remove_tree_contents(&self.temp_fd);
            let _ = unlinkat(
                &self.resources_fd,
                self.temp_name.as_str(),
                AtFlags::REMOVEDIR,
            );
        }
    }
}

fn check_parent(directory: &OwnedFd, uid: u32) -> Result<(), InstallTargetError> {
    let stat = fstat(directory).map_err(|_| InstallTargetError::UnsafeParent)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != uid
        || stat.st_mode & 0o022 != 0
    {
        return Err(InstallTargetError::UnsafeParent);
    }
    Ok(())
}

fn is_empty_dir(directory: &OwnedFd) -> bool {
    let Ok(mut reader) = Dir::read_from(directory) else {
        return false;
    };
    while let Some(entry) = reader.read() {
        let Ok(entry) = entry else { return false };
        let name = entry.file_name().to_bytes();
        if name != b"." && name != b".." {
            return false;
        }
    }
    true
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

#[allow(dead_code)]
fn _assert_os_str(_: &OsStr) {}
