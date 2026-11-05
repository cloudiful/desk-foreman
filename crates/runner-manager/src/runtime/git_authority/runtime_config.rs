//! Structural trusted-Git availability.
//!
//! This module never starts Docker, runs a helper, creates a path, changes a
//! mode, or probes a daemon. It classifies configured state only; every
//! successful result is explicitly unverified and grants nothing.

use std::path::{Path, PathBuf};

use runner_protocol::{GitRuntimeAvailability, GitRuntimeReason, GitRuntimeStatus};
use rustix::fs::{AtFlags, FileType, Mode, OFlags, fstat, open, openat, statat};
use rustix::io::Errno;

use crate::config::{RunnerBackendKind, RunnerManagerConfig};

use super::helper::{GitVerifyConfigError, GitVerifyLimits};

const DIRECTORY_OPEN_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const PRIVATE_ROOT_MODE: u32 = 0o700;
const PRIVATE_ROOT_MODE_MASK: u32 = 0o7777;
const WRITABLE_MODE_MASK: u32 = 0o022;

pub(crate) fn check_git_runtime(
    config: &RunnerManagerConfig,
    euid: Option<u32>,
) -> GitRuntimeAvailability {
    let Some(euid) = euid else {
        return invalid(GitRuntimeReason::ManagerUidUnavailable);
    };
    if euid == 0 {
        return invalid(GitRuntimeReason::ManagerRunningAsRoot);
    }
    if config.backend != RunnerBackendKind::Docker {
        return invalid(GitRuntimeReason::BackendUnsupported);
    }
    if config.git_image.is_none() {
        return unconfigured(GitRuntimeReason::ImageMissing);
    }
    if let Err(error) = GitVerifyLimits::from_config(config) {
        return invalid(match error {
            GitVerifyConfigError::UnpinnedImage => GitRuntimeReason::ImageNotDigestPinned,
            GitVerifyConfigError::InvalidImage => GitRuntimeReason::ImageInvalid,
            _ => GitRuntimeReason::LimitsInvalid,
        });
    }
    let Some(private_root) = config.git_private_root.as_deref() else {
        return unconfigured(GitRuntimeReason::PrivateRootMissing);
    };
    let normalized = match validate_private_root(private_root, euid) {
        Ok(normalized) => normalized,
        Err(reason) => return invalid(reason),
    };
    let forbidden = match forbidden_roots(config) {
        Ok(forbidden) => forbidden,
        Err(reason) => return invalid(reason),
    };
    if forbidden
        .iter()
        .any(|root| private_overlaps(&normalized, root))
    {
        return invalid(GitRuntimeReason::PrivateRootOverlapsWorkspace);
    }
    if let Err(reason) = check_docker_host(
        config.docker_host.as_deref(),
        config.git_host_private_root.as_deref(),
    ) {
        return invalid(reason);
    }
    GitRuntimeAvailability {
        status: GitRuntimeStatus::ConfiguredUnverified,
        reason: None,
    }
}

fn invalid(reason: GitRuntimeReason) -> GitRuntimeAvailability {
    GitRuntimeAvailability {
        status: GitRuntimeStatus::Invalid,
        reason: Some(reason),
    }
}

fn unconfigured(reason: GitRuntimeReason) -> GitRuntimeAvailability {
    GitRuntimeAvailability {
        status: GitRuntimeStatus::Unconfigured,
        reason: Some(reason),
    }
}

pub(crate) fn validate_private_root(root: &Path, euid: u32) -> Result<PathBuf, GitRuntimeReason> {
    let normalized = normalize_path(root).map_err(|error| match error {
        PathSpellingError::NotAbsolute => GitRuntimeReason::PrivateRootNotAbsolute,
        PathSpellingError::NotNormalized => GitRuntimeReason::PrivateRootNotNormalized,
    })?;
    let mut components = normalized.components().peekable();
    let mut current = open("/", DIRECTORY_OPEN_FLAGS, Mode::empty()).map_err(map_private_open)?;
    let mut saw_component = false;
    while let Some(component) = components.next() {
        let std::path::Component::Normal(name) = component else {
            continue;
        };
        let is_final = components.peek().is_none();
        let name = name
            .to_str()
            .ok_or(GitRuntimeReason::PrivateRootNotNormalized)?;
        let stat = statat(&current, name, AtFlags::SYMLINK_NOFOLLOW).map_err(map_private_stat)?;
        match FileType::from_raw_mode(stat.st_mode) {
            FileType::Symlink => return Err(GitRuntimeReason::PrivateRootSymlink),
            FileType::Directory => {}
            _ => return Err(GitRuntimeReason::PrivateRootNotDirectory),
        }
        classify_directory_identity(stat.st_uid, stat.st_mode, euid, is_final)?;
        let next = openat(&current, name, DIRECTORY_OPEN_FLAGS, Mode::empty())
            .map_err(map_private_open)?;
        let opened = fstat(&next).map_err(|_| GitRuntimeReason::PrivateRootUnavailable)?;
        if FileType::from_raw_mode(opened.st_mode) != FileType::Directory {
            return Err(GitRuntimeReason::PrivateRootNotDirectory);
        }
        current = next;
        saw_component = true;
    }
    if !saw_component {
        let stat = fstat(&current).map_err(|_| GitRuntimeReason::PrivateRootUnavailable)?;
        classify_directory_identity(stat.st_uid, stat.st_mode, euid, true)?;
    }
    Ok(normalized)
}

pub(super) fn classify_directory_identity(
    uid: u32,
    mode: u32,
    euid: u32,
    is_final: bool,
) -> Result<(), GitRuntimeReason> {
    if is_final {
        if uid != euid {
            return Err(GitRuntimeReason::PrivateRootNotOwned);
        }
        if mode & PRIVATE_ROOT_MODE_MASK != PRIVATE_ROOT_MODE {
            return Err(GitRuntimeReason::PrivateRootNotPrivate);
        }
        return Ok(());
    }
    if uid != 0 && uid != euid {
        return Err(GitRuntimeReason::PrivateRootNotOwned);
    }
    if mode & WRITABLE_MODE_MASK != 0 {
        return Err(GitRuntimeReason::PrivateRootAncestorUnsafe);
    }
    Ok(())
}

fn map_private_stat(error: Errno) -> GitRuntimeReason {
    match error {
        Errno::LOOP => GitRuntimeReason::PrivateRootSymlink,
        Errno::NOTDIR => GitRuntimeReason::PrivateRootNotDirectory,
        Errno::NOENT | Errno::ACCESS | Errno::PERM => GitRuntimeReason::PrivateRootUnavailable,
        _ => GitRuntimeReason::PrivateRootUnavailable,
    }
}

fn map_private_open(error: Errno) -> GitRuntimeReason {
    match error {
        Errno::LOOP => GitRuntimeReason::PrivateRootSymlink,
        Errno::NOTDIR => GitRuntimeReason::PrivateRootNotDirectory,
        Errno::NOENT | Errno::ACCESS | Errno::PERM => GitRuntimeReason::PrivateRootUnavailable,
        _ => GitRuntimeReason::PrivateRootUnavailable,
    }
}

struct ForbiddenRoot {
    lexical: PathBuf,
    canonical: Option<PathBuf>,
}

fn forbidden_roots(config: &RunnerManagerConfig) -> Result<Vec<ForbiddenRoot>, GitRuntimeReason> {
    let mut candidates: Vec<&Path> = vec![&config.workspace_root, &config.host_workspace_root];
    if let Some(root) = config.native_task_root.as_deref() {
        candidates.push(root);
    }
    if let Some(root) = config.native_task_host_root.as_deref() {
        candidates.push(root);
    }

    let mut roots = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let lexical =
            normalize_path(candidate).map_err(|_| GitRuntimeReason::WorkspaceRootUnproven)?;
        let canonical = match std::fs::symlink_metadata(&lexical) {
            Ok(_) => Some(
                std::fs::canonicalize(&lexical)
                    .map_err(|_| GitRuntimeReason::WorkspaceRootUnproven)?,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err(GitRuntimeReason::WorkspaceRootUnproven),
        };
        roots.push(ForbiddenRoot { lexical, canonical });
    }
    Ok(roots)
}

fn private_overlaps(private: &Path, forbidden: &ForbiddenRoot) -> bool {
    overlaps(private, &forbidden.lexical)
        || forbidden
            .canonical
            .as_deref()
            .is_some_and(|canonical| overlaps(private, canonical))
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PathSpellingError {
    NotAbsolute,
    NotNormalized,
}

fn normalize_path(path: &Path) -> Result<PathBuf, PathSpellingError> {
    if !path.is_absolute() {
        return Err(PathSpellingError::NotAbsolute);
    }
    let Some(text) = path.to_str() else {
        return Err(PathSpellingError::NotNormalized);
    };
    if text.contains('\\') || text.contains(',') || text.chars().any(char::is_control) {
        return Err(PathSpellingError::NotNormalized);
    }
    if text == "/" {
        return Ok(PathBuf::from("/"));
    }
    if text.ends_with('/') {
        return Err(PathSpellingError::NotNormalized);
    }
    for component in text[1..].split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(PathSpellingError::NotNormalized);
        }
    }
    Ok(PathBuf::from(text))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DockerEndpointKind {
    Local,
    Remote,
}

fn check_docker_host(
    docker_host: Option<&str>,
    host_private_root: Option<&Path>,
) -> Result<(), GitRuntimeReason> {
    let kind = classify_docker_host(docker_host)?;
    if let Some(root) = host_private_root {
        normalize_path(root).map_err(|error| match error {
            PathSpellingError::NotAbsolute => GitRuntimeReason::HostRootNotAbsolute,
            PathSpellingError::NotNormalized => GitRuntimeReason::HostRootNotNormalized,
        })?;
    }
    if kind == DockerEndpointKind::Remote && host_private_root.is_none() {
        return Err(GitRuntimeReason::HostRootRequiredForRemoteDaemon);
    }
    Ok(())
}

fn classify_docker_host(docker_host: Option<&str>) -> Result<DockerEndpointKind, GitRuntimeReason> {
    let Some(raw) = docker_host else {
        return Ok(DockerEndpointKind::Local);
    };
    let value = raw.trim();
    if value.is_empty() || value.starts_with("unix://") {
        return Ok(DockerEndpointKind::Local);
    }
    for prefix in ["tcp://", "http://", "https://", "ssh://"] {
        if let Some(rest) = value.strip_prefix(prefix) {
            if rest.is_empty() {
                return Err(GitRuntimeReason::DockerHostUnsupported);
            }
            return Ok(DockerEndpointKind::Remote);
        }
    }
    if !value.contains('/')
        && let Some((host, port)) = value.split_once(':')
        && !host.is_empty()
        && !port.is_empty()
        && port.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Ok(DockerEndpointKind::Remote);
    }
    Err(GitRuntimeReason::DockerHostUnsupported)
}
