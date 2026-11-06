//! Bounded, fd-relative validation and extraction of one install tar stream.
//!
//! The manager never runs host Git on received objects. The archive is parsed
//! twice with the mature `tar` crate: a raw pass validates every record before
//! any payload is trusted, and a resolved pass extracts files, directories and
//! symlinks relative to a pinned temporary directory descriptor.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};
use std::os::fd::OwnedFd;

use runner_protocol::GitObjectFormat;
use rustix::fs::{AtFlags, FileType, Mode, OFlags, fstat, mkdirat, openat, statat, symlinkat};
use tar::{Archive, EntryType};

pub(crate) const MAX_PATH_BYTES: usize = 4096;
const MAX_LINK_BYTES: usize = 4096;
const BLOCK: u64 = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallArchiveError {
    Truncated,
    InvalidHeader,
    InvalidEntry,
    UnsupportedEntry,
    InvalidPath,
    DuplicatePath,
    PrefixConflict,
    MetadataLimit,
    EntryLimit,
    WorktreeLimit,
    GitLimit,
    FileFailed,
    MetadataMismatch,
}

impl std::fmt::Display for InstallArchiveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Truncated => "install archive is truncated",
            Self::InvalidHeader => "install archive header is invalid",
            Self::InvalidEntry => "install archive entry is invalid",
            Self::UnsupportedEntry => "install archive entry type is unsupported",
            Self::InvalidPath => "install archive path is invalid",
            Self::DuplicatePath => "install archive path is duplicated",
            Self::PrefixConflict => "install archive path prefix conflicts",
            Self::MetadataLimit => "install archive metadata exceeds its limit",
            Self::EntryLimit => "install archive entry count exceeds its limit",
            Self::WorktreeLimit => "install archive worktree exceeds its limit",
            Self::GitLimit => "install archive git data exceeds its limit",
            Self::FileFailed => "install archive file operation failed",
            Self::MetadataMismatch => "installed git metadata does not match",
        })
    }
}

impl std::error::Error for InstallArchiveError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ArchiveLimits {
    pub(crate) worktree_bytes: u64,
    pub(crate) git_bytes: u64,
    pub(crate) metadata_bytes: u64,
    pub(crate) max_entries: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ArchiveSummary {
    pub(crate) worktree_bytes: u64,
    pub(crate) git_bytes: u64,
    pub(crate) entries: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NodeKind {
    Directory,
    File,
    Symlink,
}

pub(crate) fn extract(
    bytes: &[u8],
    root: &OwnedFd,
    limits: &ArchiveLimits,
) -> Result<ArchiveSummary, InstallArchiveError> {
    let raw_expected = raw_pass(bytes, limits)?;

    let mut archive = Archive::new(Cursor::new(bytes));
    let mut entries = archive
        .entries()
        .map_err(|_| InstallArchiveError::InvalidHeader)?;
    let mut nodes: BTreeMap<Vec<u8>, NodeKind> = BTreeMap::new();
    let mut dirs: BTreeSet<Vec<u8>> = BTreeSet::new();
    let mut summary = ArchiveSummary {
        worktree_bytes: 0,
        git_bytes: 0,
        entries: 0,
    };
    while let Some(mut entry) = entries
        .next()
        .transpose()
        .map_err(|_| InstallArchiveError::InvalidHeader)?
    {
        summary.entries = summary
            .entries
            .checked_add(1)
            .ok_or(InstallArchiveError::EntryLimit)?;
        if summary.entries > raw_expected || summary.entries > limits.max_entries {
            return Err(InstallArchiveError::EntryLimit);
        }
        validate_pax(&mut entry, limits)?;
        let kind = match entry.header().entry_type() {
            EntryType::Regular | EntryType::Continuous => NodeKind::File,
            EntryType::Directory => NodeKind::Directory,
            EntryType::Symlink => NodeKind::Symlink,
            _ => return Err(InstallArchiveError::UnsupportedEntry),
        };
        let components = normalize_path(&entry.path_bytes(), kind)?;
        if components.is_empty() {
            if kind != NodeKind::Directory {
                return Err(InstallArchiveError::InvalidPath);
            }
            continue;
        }
        let key = components.join(&b'/');
        let is_git = components.first().is_some_and(|value| value == b".git");
        if is_git && kind == NodeKind::Symlink {
            return Err(InstallArchiveError::InvalidEntry);
        }
        if components.len() == 1 && is_git && kind != NodeKind::Directory {
            return Err(InstallArchiveError::InvalidEntry);
        }
        match kind {
            NodeKind::Directory => {
                if nodes.contains_key(&key) {
                    return Err(InstallArchiveError::DuplicatePath);
                }
                ensure_ancestors(root, &components, &mut nodes, &mut dirs, limits)?;
                create_dir(root, &components)?;
                dirs.insert(key.clone());
                nodes.insert(key, NodeKind::Directory);
            }
            NodeKind::File | NodeKind::Symlink => {
                if nodes.contains_key(&key) {
                    return Err(InstallArchiveError::DuplicatePath);
                }
                for prefix in path_prefixes(&key) {
                    if matches!(nodes.get(prefix), Some(NodeKind::File | NodeKind::Symlink)) {
                        return Err(InstallArchiveError::PrefixConflict);
                    }
                }
                if nodes
                    .range(key.clone()..)
                    .next()
                    .is_some_and(|(candidate, _)| candidate.starts_with(&descendant_prefix(&key)))
                {
                    return Err(InstallArchiveError::PrefixConflict);
                }
                ensure_ancestors(root, &components, &mut nodes, &mut dirs, limits)?;
                let parent = open_parent(root, &components)?;
                let name = components.last().expect("component");
                match kind {
                    NodeKind::File => {
                        let size = entry.size();
                        if size > limits.worktree_bytes.max(limits.git_bytes) {
                            return Err(if is_git {
                                InstallArchiveError::GitLimit
                            } else {
                                InstallArchiveError::WorktreeLimit
                            });
                        }
                        if is_git {
                            summary.git_bytes = summary
                                .git_bytes
                                .checked_add(size)
                                .ok_or(InstallArchiveError::GitLimit)?;
                            if summary.git_bytes > limits.git_bytes {
                                return Err(InstallArchiveError::GitLimit);
                            }
                        } else {
                            summary.worktree_bytes = summary
                                .worktree_bytes
                                .checked_add(size)
                                .ok_or(InstallArchiveError::WorktreeLimit)?;
                            if summary.worktree_bytes > limits.worktree_bytes {
                                return Err(InstallArchiveError::WorktreeLimit);
                            }
                        }
                        let mode = entry.header().mode().unwrap_or(0o644) & 0o777;
                        if !matches!(mode, 0o644 | 0o755) {
                            return Err(InstallArchiveError::InvalidEntry);
                        }
                        let fd = openat(
                            &parent,
                            name.as_slice(),
                            OFlags::WRONLY
                                | OFlags::CREATE
                                | OFlags::EXCL
                                | OFlags::NOFOLLOW
                                | OFlags::CLOEXEC,
                            Mode::from_bits_truncate(mode),
                        )
                        .map_err(|_| InstallArchiveError::FileFailed)?;
                        let mut file = std::fs::File::from(fd);
                        std::io::copy(&mut entry, &mut file)
                            .map_err(|_| InstallArchiveError::FileFailed)?;
                    }
                    NodeKind::Symlink => {
                        let target = entry
                            .link_name_bytes()
                            .ok_or(InstallArchiveError::InvalidEntry)?;
                        if target.is_empty() || target.len() > MAX_LINK_BYTES || target.contains(&0)
                        {
                            return Err(InstallArchiveError::InvalidPath);
                        }
                        symlinkat(target.as_ref(), &parent, name.as_slice())
                            .map_err(|_| InstallArchiveError::FileFailed)?;
                    }
                    NodeKind::Directory => unreachable!(),
                }
                nodes.insert(key, kind);
            }
        }
    }
    if summary.entries != raw_expected {
        return Err(InstallArchiveError::InvalidEntry);
    }
    Ok(summary)
}

fn raw_pass(bytes: &[u8], limits: &ArchiveLimits) -> Result<u64, InstallArchiveError> {
    let mut archive = Archive::new(Cursor::new(bytes));
    let mut entries = archive
        .entries()
        .map_err(|_| InstallArchiveError::InvalidHeader)?
        .raw(true);
    let mut expected = 0u64;
    let mut end = 0u64;
    while let Some(entry) = entries
        .next()
        .transpose()
        .map_err(|_| InstallArchiveError::InvalidHeader)?
    {
        let position = entry.raw_file_position();
        let size = entry.size();
        let padded = size
            .checked_add(BLOCK - 1)
            .and_then(|value| value.checked_div(BLOCK))
            .and_then(|blocks| blocks.checked_mul(BLOCK))
            .and_then(|padded| position.checked_add(padded))
            .ok_or(InstallArchiveError::MetadataLimit)?;
        end = end.max(padded);
        match entry.header().entry_type() {
            EntryType::Regular | EntryType::Directory | EntryType::Symlink => {
                expected = expected
                    .checked_add(1)
                    .ok_or(InstallArchiveError::EntryLimit)?;
                if expected > limits.max_entries {
                    return Err(InstallArchiveError::EntryLimit);
                }
            }
            EntryType::GNULongName | EntryType::GNULongLink | EntryType::XHeader => {
                if size > limits.metadata_bytes {
                    return Err(InstallArchiveError::MetadataLimit);
                }
            }
            _ => return Err(InstallArchiveError::UnsupportedEntry),
        }
    }
    let len = bytes.len() as u64;
    if end > len || len - end < 2 * BLOCK {
        return Err(InstallArchiveError::Truncated);
    }
    if bytes[end as usize..].iter().any(|byte| *byte != 0) {
        return Err(InstallArchiveError::InvalidEntry);
    }
    Ok(expected)
}

fn validate_pax<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    limits: &ArchiveLimits,
) -> Result<(), InstallArchiveError> {
    let Some(extensions) = entry
        .pax_extensions()
        .map_err(|_| InstallArchiveError::InvalidHeader)?
    else {
        return Ok(());
    };
    for extension in extensions {
        let extension = extension.map_err(|_| InstallArchiveError::InvalidHeader)?;
        let key = extension.key_bytes();
        let value = extension.value_bytes();
        let lower = key.to_ascii_lowercase();
        let allowed = matches!(
            key,
            b"path"
                | b"linkpath"
                | b"mtime"
                | b"atime"
                | b"ctime"
                | b"uid"
                | b"gid"
                | b"uname"
                | b"gname"
        );
        if !allowed || lower.windows(4).any(|window| window == b"size") {
            return Err(InstallArchiveError::InvalidEntry);
        }
        if matches!(key, b"path" | b"linkpath")
            && (value.is_empty() || value.len() > MAX_PATH_BYTES)
        {
            return Err(InstallArchiveError::MetadataLimit);
        }
        if value.len() as u64 > limits.metadata_bytes {
            return Err(InstallArchiveError::MetadataLimit);
        }
    }
    Ok(())
}

fn normalize_path(raw: &[u8], kind: NodeKind) -> Result<Vec<Vec<u8>>, InstallArchiveError> {
    if raw.is_empty() || raw.contains(&0) || raw[0] == b'/' || raw.len() > MAX_PATH_BYTES {
        return Err(InstallArchiveError::InvalidPath);
    }
    let mut path = raw;
    if let Some(rest) = path.strip_prefix(b"./") {
        path = rest;
    }
    let mut trimmed = path;
    while trimmed.last() == Some(&b'/') {
        if kind != NodeKind::Directory {
            return Err(InstallArchiveError::InvalidPath);
        }
        trimmed = &trimmed[..trimmed.len() - 1];
    }
    if trimmed.is_empty() || trimmed == b"." {
        return Ok(Vec::new());
    }
    let mut components = Vec::new();
    for component in trimmed.split(|byte| *byte == b'/') {
        if component.is_empty() || component == b"." || component == b".." || component.len() > 255
        {
            return Err(InstallArchiveError::InvalidPath);
        }
        components.push(component.to_vec());
    }
    Ok(components)
}

fn path_prefixes(key: &[u8]) -> impl Iterator<Item = &[u8]> {
    key.iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'/')
        .map(|(index, _)| &key[..index])
}

fn descendant_prefix(key: &[u8]) -> Vec<u8> {
    let mut prefix = key.to_vec();
    prefix.push(b'/');
    prefix
}

fn ensure_ancestors(
    root: &OwnedFd,
    components: &[Vec<u8>],
    nodes: &mut BTreeMap<Vec<u8>, NodeKind>,
    dirs: &mut BTreeSet<Vec<u8>>,
    limits: &ArchiveLimits,
) -> Result<(), InstallArchiveError> {
    if components.len() <= 1 {
        return Ok(());
    }
    let mut current = root
        .try_clone()
        .map_err(|_| InstallArchiveError::FileFailed)?;
    let mut prefix = Vec::new();
    for component in &components[..components.len() - 1] {
        if !prefix.is_empty() {
            prefix.push(b'/');
        }
        prefix.extend_from_slice(component);
        if let Some(kind) = nodes.get(&prefix)
            && *kind != NodeKind::Directory
        {
            return Err(InstallArchiveError::PrefixConflict);
        }
        if dirs.insert(prefix.clone()) {
            if dirs.len() as u64 > limits.max_entries {
                return Err(InstallArchiveError::EntryLimit);
            }
            match mkdirat(
                &current,
                component.as_slice(),
                Mode::from_bits_truncate(0o755),
            ) {
                Ok(()) => {}
                Err(rustix::io::Errno::EXIST) => {}
                Err(_) => return Err(InstallArchiveError::FileFailed),
            }
        }
        let next = openat(
            &current,
            component.as_slice(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| InstallArchiveError::PrefixConflict)?;
        current = next;
    }
    Ok(())
}

fn create_dir(root: &OwnedFd, components: &[Vec<u8>]) -> Result<(), InstallArchiveError> {
    let parent = open_parent(root, components)?;
    let name = components.last().expect("component");
    match mkdirat(&parent, name.as_slice(), Mode::from_bits_truncate(0o755)) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => Ok(()),
        Err(_) => Err(InstallArchiveError::FileFailed),
    }
}

fn open_parent(root: &OwnedFd, components: &[Vec<u8>]) -> Result<OwnedFd, InstallArchiveError> {
    let mut current = root
        .try_clone()
        .map_err(|_| InstallArchiveError::FileFailed)?;
    for component in &components[..components.len() - 1] {
        let next = openat(
            &current,
            component.as_slice(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|_| InstallArchiveError::FileFailed)?;
        current = next;
    }
    let stat = fstat(&current).map_err(|_| InstallArchiveError::FileFailed)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
        return Err(InstallArchiveError::FileFailed);
    }
    Ok(current)
}

/// Validate the installed `.git` metadata without running host Git.
pub(crate) fn inspect_git(
    root: &OwnedFd,
    branch: &str,
    head: &str,
    format: GitObjectFormat,
    metadata_max: u64,
) -> Result<(), InstallArchiveError> {
    let head_file =
        read_file(root, &[b".git", b"HEAD"], 256)?.ok_or(InstallArchiveError::MetadataMismatch)?;
    let expected_head = format!("ref: refs/heads/{branch}");
    if String::from_utf8_lossy(&head_file).trim() != expected_head {
        return Err(InstallArchiveError::MetadataMismatch);
    }
    let ref_path = format!("refs/heads/{branch}");
    let ref_bytes = read_file(root, &[b".git", ref_path.as_bytes()], 256)?
        .or_else(|| {
            read_file(root, &[b".git", b"packed-refs"], metadata_max)
                .ok()
                .flatten()
                .and_then(|packed| packed_head(&packed, &ref_path))
        })
        .ok_or(InstallArchiveError::MetadataMismatch)?;
    if String::from_utf8_lossy(&ref_bytes).trim() != head {
        return Err(InstallArchiveError::MetadataMismatch);
    }
    let config = read_file(root, &[b".git", b"config"], metadata_max)?
        .ok_or(InstallArchiveError::MetadataMismatch)?;
    let config = String::from_utf8_lossy(&config).to_ascii_lowercase();
    if config.contains("alternates")
        || config.contains("[include")
        || config.contains("[remote")
        || config.contains("url =")
        || config.contains("url=")
        || config.contains("proxy")
        || config.contains("credential")
        || config.contains("[http")
        || config.contains("bundle")
    {
        return Err(InstallArchiveError::MetadataMismatch);
    }
    let configured_format = config.lines().find_map(|line| {
        let (key, value) = line.trim().split_once('=')?;
        (key.trim() == "objectformat").then(|| value.trim())
    });
    let valid_format = match format {
        GitObjectFormat::Sha1 => configured_format.is_none() || configured_format == Some("sha1"),
        GitObjectFormat::Sha256 => configured_format == Some("sha256"),
    };
    if !valid_format {
        return Err(InstallArchiveError::MetadataMismatch);
    }
    if read_file(
        root,
        &[b".git", b"objects", b"info", b"alternates"],
        metadata_max,
    )?
    .is_some()
        || read_file(root, &[b".git", b"commondir"], metadata_max)?.is_some()
    {
        return Err(InstallArchiveError::MetadataMismatch);
    }
    if let Ok(git_dir) = openat(
        root,
        c".git",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        match statat(&git_dir, c"worktrees", AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => return Err(InstallArchiveError::MetadataMismatch),
            Err(rustix::io::Errno::NOENT) => {}
            Err(_) => return Err(InstallArchiveError::MetadataMismatch),
        }
    }
    Ok(())
}

fn packed_head(packed: &[u8], reference: &str) -> Option<Vec<u8>> {
    for line in packed.split(|byte| *byte == b'\n') {
        let line = line.strip_prefix(b"#").unwrap_or(line).trim_ascii();
        let mut fields = line.split(|byte| *byte == b' ');
        let value = fields.next()?;
        let name = fields.next()?;
        if name == reference.as_bytes() && value.len() >= 40 {
            return Some(value.to_vec());
        }
    }
    None
}

fn read_file(
    root: &OwnedFd,
    components: &[&[u8]],
    max: u64,
) -> Result<Option<Vec<u8>>, InstallArchiveError> {
    let mut current = root
        .try_clone()
        .map_err(|_| InstallArchiveError::FileFailed)?;
    let mut prefix = Vec::new();
    for component in &components[..components.len() - 1] {
        if !prefix.is_empty() {
            prefix.push(b'/');
        }
        prefix.extend_from_slice(component);
        let next = openat(
            &current,
            *component,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        );
        match next {
            Ok(fd) => current = fd,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(_) => return Err(InstallArchiveError::MetadataMismatch),
        }
    }
    let name = components.last().expect("component");
    let fd = match openat(
        &current,
        *name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(_) => return Err(InstallArchiveError::MetadataMismatch),
    };
    let stat = fstat(&fd).map_err(|_| InstallArchiveError::MetadataMismatch)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile || stat.st_size as u64 > max {
        return Err(InstallArchiveError::MetadataMismatch);
    }
    let mut file = std::fs::File::from(fd);
    let mut data = Vec::with_capacity(stat.st_size as usize);
    file.read_to_end(&mut data)
        .map_err(|_| InstallArchiveError::MetadataMismatch)?;
    Ok(Some(data))
}

#[allow(dead_code)]
fn _assert_file_type_ext(_: &std::fs::FileType) -> bool {
    false
}
