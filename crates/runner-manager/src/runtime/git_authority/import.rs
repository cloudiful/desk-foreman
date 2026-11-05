//! Credential-free raw Git snapshot staging.
//!
//! The importer reads only a narrow, unverified slice of Git's on-disk data
//! from a manager-pinned workspace descriptor and copies it into a
//! manager-private quarantine directory. It never runs Git, parses config,
//! resolves arbitrary refs, follows a `.git` pointer or symlinked path, or
//! treats the result as a verified commit. Bounds are explicit and required.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::path::{Component, Path, PathBuf};

use rustix::fs::{
    AtFlags, Dir, FileType, Mode, OFlags, fchmod, fstat, mkdirat, openat, statat, unlinkat,
};
use rustix::io::Errno;

use super::limits::GitImportLimits;

const DIRECTORY_OPEN_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const CANDIDATE_OPEN_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::NONBLOCK)
    .union(OFlags::CLOEXEC);
const OUTPUT_FILE_FLAGS: OFlags = OFlags::WRONLY
    .union(OFlags::CREATE)
    .union(OFlags::EXCL)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

const PRIVATE_DIR_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;
const MAX_CHECKOUT_COMPONENTS: usize = 64;
const MAX_BRANCH_COMPONENTS: usize = 64;
const MAX_BRANCH_BYTES: usize = 1024;
const COPY_BUFFER_BYTES: usize = 64 * 1024;

/// Request to stage raw Git data from one checkout.
///
/// Both root descriptors are opened by the manager. The checkout location is
/// resolved component by component from `workspace_root` with no-follow
/// directory opens; it is never treated as a trusted path string.
#[derive(Debug)]
pub(crate) struct RawGitSnapshotRequest {
    pub(crate) workspace_root: OwnedFd,
    pub(crate) quarantine_root: OwnedFd,
    pub(crate) checkout_relative: PathBuf,
    pub(crate) pinned_branch: String,
    pub(crate) expected_head: String,
}

/// Result of raw staging. This is explicitly unvalidated data, not a commit
/// and not an admission grant; cryptographic object and ancestry validation
/// belongs to a later phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawGitSnapshot {
    /// Manager-generated directory name under the quarantine root.
    pub(crate) snapshot_name: String,
    pub(crate) object_files: u64,
    pub(crate) object_bytes: u64,
    pub(crate) metadata_files: u64,
    pub(crate) metadata_bytes: u64,
    pub(crate) captured_head: String,
    pub(crate) pinned_branch: String,
}

/// Fixed, non-secret failure classes for raw snapshot staging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum GitImportError {
    #[error("git import limits are invalid")]
    InvalidLimits,
    #[error("checkout location is invalid")]
    InvalidCheckoutLocation,
    #[error("branch metadata is invalid")]
    InvalidBranch,
    #[error("expected head is invalid")]
    InvalidExpectedHead,
    #[error("git layout is unsupported")]
    UnsupportedLayout,
    #[error("unsafe filesystem entry")]
    UnsafeEntry,
    #[error("git import limit exceeded")]
    LimitExceeded,
    #[error("git head does not match the expected head")]
    HeadMismatch,
    #[error("git head changed during import")]
    HeadChanged,
    #[error("quarantine destination is unsafe")]
    UnsafeDestination,
    #[error("git import io failure")]
    Io,
}

pub(crate) fn stage_raw_git_snapshot(
    request: RawGitSnapshotRequest,
    limits: GitImportLimits,
) -> Result<RawGitSnapshot, GitImportError> {
    let limits = limits.validate()?;
    let hash_len = expected_hash_len(&request.expected_head)?;
    let branch_components = validate_branch(&request.pinned_branch)?;
    let checkout_components = validate_checkout(&request.checkout_relative)?;

    require_directory(&request.workspace_root)?;
    require_directory(&request.quarantine_root)?;

    let checkout = open_checkout(&request.workspace_root, &checkout_components)?;

    #[cfg(test)]
    seam::fire(seam::Step::OpenGitDir);

    let git = open_expected_dir(&checkout, ".git")?;
    reject_present(&git, "commondir")?;
    reject_present(&git, "worktrees")?;

    let objects = open_expected_dir(&git, "objects")?;
    reject_alternates(&objects)?;

    let mut budget = MetadataBudget::new(limits.max_metadata_bytes);
    let capture = read_head_capture(
        &git,
        &request.pinned_branch,
        &branch_components,
        &request.expected_head,
        hash_len,
        &mut budget,
    )?;

    #[cfg(test)]
    let name_override = seam::snapshot_name_override();
    #[cfg(not(test))]
    let name_override: Option<String> = None;

    #[cfg(test)]
    seam::fire(seam::Step::SnapshotAllocate);

    let (snapshot_name, snapshot_dir) = allocate_snapshot(&request.quarantine_root, name_override)?;
    let mut pending = PendingSnapshot {
        parent: &request.quarantine_root,
        name: snapshot_name.clone(),
        dir: snapshot_dir,
        committed: false,
    };

    let mut file_count = 0_u64;
    let mut metadata_bytes = 0_u64;
    write_metadata_file(
        &pending.dir,
        "HEAD",
        &capture.raw_head,
        &mut file_count,
        &mut metadata_bytes,
        limits,
    )?;
    if let Some(raw_ref) = &capture.raw_ref {
        let refs = create_dir(&pending.dir, "refs")?;
        let heads = create_dir(&refs, "heads")?;
        let mut current = heads;
        for (index, component) in branch_components.iter().enumerate() {
            if index + 1 == branch_components.len() {
                write_metadata_file(
                    &current,
                    component,
                    raw_ref,
                    &mut file_count,
                    &mut metadata_bytes,
                    limits,
                )?;
            } else {
                current = create_dir(&current, component)?;
            }
        }
    }

    let mut object_files = 0_u64;
    let mut object_bytes = 0_u64;
    let mut entries = 0_u64;
    let out_objects = create_dir(&pending.dir, "objects")?;
    {
        let mut counters = StageCounters {
            entries: &mut entries,
            file_count: &mut file_count,
            object_files: &mut object_files,
            object_bytes: &mut object_bytes,
        };
        stage_objects(&objects, &out_objects, hash_len, limits, &mut counters)?;
    }

    #[cfg(test)]
    seam::fire(seam::Step::RecheckHead);

    let mut recheck_budget = MetadataBudget::new(limits.max_metadata_bytes);
    let recheck = read_head_capture(
        &git,
        &request.pinned_branch,
        &branch_components,
        &request.expected_head,
        hash_len,
        &mut recheck_budget,
    )
    .map_err(|_| GitImportError::HeadChanged)?;
    if recheck.raw_head != capture.raw_head
        || recheck.raw_ref != capture.raw_ref
        || recheck.captured_head != capture.captured_head
    {
        return Err(GitImportError::HeadChanged);
    }

    pending.committed = true;
    Ok(RawGitSnapshot {
        snapshot_name,
        object_files,
        object_bytes,
        metadata_files: file_count - object_files,
        metadata_bytes,
        captured_head: capture.captured_head,
        pinned_branch: request.pinned_branch,
    })
}

fn require_directory(fd: &OwnedFd) -> Result<(), GitImportError> {
    let stat = fstat(fd).map_err(|_| GitImportError::Io)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
        return Err(GitImportError::InvalidCheckoutLocation);
    }
    Ok(())
}

fn validate_checkout(path: &Path) -> Result<Vec<String>, GitImportError> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(GitImportError::InvalidCheckoutLocation);
    }
    let mut components = Vec::new();
    for component in path.components() {
        let Component::Normal(value) = component else {
            return Err(GitImportError::InvalidCheckoutLocation);
        };
        let text = value
            .to_str()
            .ok_or(GitImportError::InvalidCheckoutLocation)?;
        if text.is_empty()
            || text == "."
            || text == ".."
            || text.as_bytes().contains(&0)
            || text.len() > 255
        {
            return Err(GitImportError::InvalidCheckoutLocation);
        }
        components.push(text.to_owned());
    }
    if components.is_empty() || components.len() > MAX_CHECKOUT_COMPONENTS {
        return Err(GitImportError::InvalidCheckoutLocation);
    }
    Ok(components)
}

fn validate_branch(branch: &str) -> Result<Vec<String>, GitImportError> {
    if branch.is_empty() || branch.len() > MAX_BRANCH_BYTES {
        return Err(GitImportError::InvalidBranch);
    }
    if branch
        .bytes()
        .any(|byte| byte == 0 || byte == b'\n' || byte == b'\r')
    {
        return Err(GitImportError::InvalidBranch);
    }
    if branch.starts_with('/') || branch.ends_with('/') {
        return Err(GitImportError::InvalidBranch);
    }
    let mut components = Vec::new();
    for component in branch.split('/') {
        if component.is_empty() || component == "." || component == ".." || component.len() > 255 {
            return Err(GitImportError::InvalidBranch);
        }
        components.push(component.to_owned());
    }
    if components.len() > MAX_BRANCH_COMPONENTS {
        return Err(GitImportError::InvalidBranch);
    }
    Ok(components)
}

fn expected_hash_len(expected: &str) -> Result<usize, GitImportError> {
    let len = match expected.len() {
        40 => 40,
        64 => 64,
        _ => return Err(GitImportError::InvalidExpectedHead),
    };
    if !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(GitImportError::InvalidExpectedHead);
    }
    Ok(len)
}

fn open_checkout(root: &OwnedFd, components: &[String]) -> Result<OwnedFd, GitImportError> {
    let mut current: Option<OwnedFd> = None;
    for component in components {
        let parent = current.as_ref().unwrap_or(root);
        current = Some(open_expected_dir(parent, component)?);
    }
    current.ok_or(GitImportError::InvalidCheckoutLocation)
}

fn open_expected_dir<P: rustix::path::Arg>(
    parent: &OwnedFd,
    name: P,
) -> Result<OwnedFd, GitImportError> {
    let fd = openat(parent, name, DIRECTORY_OPEN_FLAGS, Mode::empty()).map_err(map_dir_open)?;
    let stat = fstat(&fd).map_err(|_| GitImportError::Io)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
        return Err(GitImportError::UnsupportedLayout);
    }
    Ok(fd)
}

fn map_dir_open(error: Errno) -> GitImportError {
    match error {
        Errno::LOOP | Errno::NOTDIR | Errno::NOENT => GitImportError::UnsupportedLayout,
        _ => GitImportError::Io,
    }
}

fn reject_present(parent: &OwnedFd, name: &str) -> Result<(), GitImportError> {
    match statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => Err(GitImportError::UnsupportedLayout),
        Err(Errno::NOENT) => Ok(()),
        Err(_) => Err(GitImportError::Io),
    }
}

fn reject_alternates(objects: &OwnedFd) -> Result<(), GitImportError> {
    let info = match statat(objects, "info", AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(Errno::NOENT) => return Ok(()),
        Err(_) => return Err(GitImportError::Io),
    };
    if FileType::from_raw_mode(info.st_mode) != FileType::Directory {
        return Err(GitImportError::UnsupportedLayout);
    }
    let info = open_expected_dir(objects, "info")?;
    reject_present(&info, "alternates")
}

fn open_candidate(parent: &OwnedFd, name: &str) -> Result<File, GitImportError> {
    let fd = openat(parent, name, CANDIDATE_OPEN_FLAGS, Mode::empty()).map_err(map_candidate)?;
    let stat = fstat(&fd).map_err(|_| GitImportError::Io)?;
    if !is_regular_candidate(FileType::from_raw_mode(stat.st_mode), stat.st_nlink) {
        return Err(GitImportError::UnsafeEntry);
    }
    Ok(File::from(fd))
}

pub(super) fn is_regular_candidate(file_type: FileType, link_count: u64) -> bool {
    file_type == FileType::RegularFile && link_count <= 1
}

fn map_candidate(error: Errno) -> GitImportError {
    match error {
        Errno::LOOP | Errno::NXIO | Errno::ISDIR => GitImportError::UnsafeEntry,
        Errno::NOENT | Errno::NOTDIR => GitImportError::UnsupportedLayout,
        _ => GitImportError::Io,
    }
}

fn create_dir(parent: &OwnedFd, name: &str) -> Result<OwnedFd, GitImportError> {
    match mkdirat(parent, name, Mode::from_bits_truncate(PRIVATE_DIR_MODE)) {
        Ok(()) => {}
        Err(Errno::EXIST | Errno::LOOP | Errno::NOTDIR) => {
            return Err(GitImportError::UnsafeDestination);
        }
        Err(_) => return Err(GitImportError::Io),
    }
    let fd = open_expected_dir(parent, name).map_err(|_| GitImportError::UnsafeDestination)?;
    fchmod(&fd, Mode::from_bits_truncate(PRIVATE_DIR_MODE)).map_err(|_| GitImportError::Io)?;
    Ok(fd)
}

fn create_output_file(parent: &OwnedFd, name: &str) -> Result<File, GitImportError> {
    let fd = openat(
        parent,
        name,
        OUTPUT_FILE_FLAGS,
        Mode::from_bits_truncate(PRIVATE_FILE_MODE),
    )
    .map_err(|error| match error {
        Errno::EXIST | Errno::LOOP | Errno::ISDIR | Errno::NOTDIR => {
            GitImportError::UnsafeDestination
        }
        _ => GitImportError::Io,
    })?;
    fchmod(&fd, Mode::from_bits_truncate(PRIVATE_FILE_MODE)).map_err(|_| GitImportError::Io)?;
    Ok(File::from(fd))
}

fn write_metadata_file(
    parent: &OwnedFd,
    name: &str,
    bytes: &[u8],
    file_count: &mut u64,
    metadata_bytes: &mut u64,
    limits: GitImportLimits,
) -> Result<(), GitImportError> {
    *file_count = file_count
        .checked_add(1)
        .ok_or(GitImportError::LimitExceeded)?;
    if *file_count > limits.max_files {
        return Err(GitImportError::LimitExceeded);
    }
    *metadata_bytes = metadata_bytes
        .checked_add(bytes.len() as u64)
        .ok_or(GitImportError::LimitExceeded)?;
    if *metadata_bytes > limits.max_metadata_bytes {
        return Err(GitImportError::LimitExceeded);
    }
    let mut file = create_output_file(parent, name)?;
    file.write_all(bytes).map_err(|_| GitImportError::Io)
}

struct StageCounters<'a> {
    entries: &'a mut u64,
    file_count: &'a mut u64,
    object_files: &'a mut u64,
    object_bytes: &'a mut u64,
}

fn stage_objects(
    source: &OwnedFd,
    destination: &OwnedFd,
    hash_len: usize,
    limits: GitImportLimits,
    counters: &mut StageCounters<'_>,
) -> Result<(), GitImportError> {
    let mut reader = Dir::read_from(source).map_err(|_| GitImportError::Io)?;
    while let Some(entry) = reader.read() {
        let entry = entry.map_err(|_| GitImportError::Io)?;
        let raw_name = entry.file_name().to_bytes();
        if raw_name == b"." || raw_name == b".." {
            continue;
        }
        *counters.entries = counters
            .entries
            .checked_add(1)
            .ok_or(GitImportError::LimitExceeded)?;
        if *counters.entries > limits.max_entries {
            return Err(GitImportError::LimitExceeded);
        }
        let name = std::str::from_utf8(raw_name).map_err(|_| GitImportError::UnsupportedLayout)?;
        if name == "pack" {
            let pack = open_expected_dir(source, "pack")?;
            let out_pack = create_dir(destination, "pack")?;
            stage_pack(&pack, &out_pack, hash_len, limits, counters)?;
        } else if name == "info" {
            continue;
        } else if is_hex(raw_name, 2) {
            #[cfg(test)]
            seam::fire(seam::Step::OpenObjectDirectory);
            let loose_dir = open_expected_dir(source, name)?;
            let out_dir = create_dir(destination, name)?;
            stage_loose_dir(&loose_dir, &out_dir, hash_len, limits, counters)?;
        } else {
            return Err(GitImportError::UnsupportedLayout);
        }
    }
    Ok(())
}

fn stage_pack(
    source: &OwnedFd,
    destination: &OwnedFd,
    hash_len: usize,
    limits: GitImportLimits,
    counters: &mut StageCounters<'_>,
) -> Result<(), GitImportError> {
    let mut reader = Dir::read_from(source).map_err(|_| GitImportError::Io)?;
    while let Some(entry) = reader.read() {
        let entry = entry.map_err(|_| GitImportError::Io)?;
        let raw_name = entry.file_name().to_bytes();
        if raw_name == b"." || raw_name == b".." {
            continue;
        }
        *counters.entries = counters
            .entries
            .checked_add(1)
            .ok_or(GitImportError::LimitExceeded)?;
        if *counters.entries > limits.max_entries {
            return Err(GitImportError::LimitExceeded);
        }
        let name = std::str::from_utf8(raw_name).map_err(|_| GitImportError::UnsupportedLayout)?;
        if !is_pack_file(name, hash_len) {
            return Err(GitImportError::UnsupportedLayout);
        }
        #[cfg(test)]
        seam::fire(seam::Step::OpenObjectFile);
        copy_object_file(source, name, destination, name, limits, counters)?;
    }
    Ok(())
}

fn stage_loose_dir(
    source: &OwnedFd,
    destination: &OwnedFd,
    hash_len: usize,
    limits: GitImportLimits,
    counters: &mut StageCounters<'_>,
) -> Result<(), GitImportError> {
    let mut reader = Dir::read_from(source).map_err(|_| GitImportError::Io)?;
    while let Some(entry) = reader.read() {
        let entry = entry.map_err(|_| GitImportError::Io)?;
        let raw_name = entry.file_name().to_bytes();
        if raw_name == b"." || raw_name == b".." {
            continue;
        }
        *counters.entries = counters
            .entries
            .checked_add(1)
            .ok_or(GitImportError::LimitExceeded)?;
        if *counters.entries > limits.max_entries {
            return Err(GitImportError::LimitExceeded);
        }
        let name = std::str::from_utf8(raw_name).map_err(|_| GitImportError::UnsupportedLayout)?;
        if !is_hex(raw_name, hash_len.saturating_sub(2)) {
            return Err(GitImportError::UnsupportedLayout);
        }
        #[cfg(test)]
        seam::fire(seam::Step::OpenObjectFile);
        copy_object_file(source, name, destination, name, limits, counters)?;
    }
    Ok(())
}

fn copy_object_file(
    source_parent: &OwnedFd,
    source_name: &str,
    destination_parent: &OwnedFd,
    destination_name: &str,
    limits: GitImportLimits,
    counters: &mut StageCounters<'_>,
) -> Result<(), GitImportError> {
    let mut source = open_candidate(source_parent, source_name)?;
    #[cfg(test)]
    seam::fire(seam::Step::CopyObjectFile);
    *counters.file_count = counters
        .file_count
        .checked_add(1)
        .ok_or(GitImportError::LimitExceeded)?;
    if *counters.file_count > limits.max_files {
        return Err(GitImportError::LimitExceeded);
    }
    let mut destination = create_output_file(destination_parent, destination_name)?;
    let mut written = 0_u64;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        let file_remaining = limits.max_file_bytes - written;
        let total_remaining = limits.max_total_bytes - *counters.object_bytes;
        let cap = file_remaining.min(total_remaining).saturating_add(1);
        let want = cap.min(COPY_BUFFER_BYTES as u64) as usize;
        let read = source
            .read(&mut buffer[..want])
            .map_err(|_| GitImportError::Io)?;
        if read == 0 {
            break;
        }
        written = written
            .checked_add(read as u64)
            .ok_or(GitImportError::LimitExceeded)?;
        *counters.object_bytes = counters
            .object_bytes
            .checked_add(read as u64)
            .ok_or(GitImportError::LimitExceeded)?;
        if written > limits.max_file_bytes || *counters.object_bytes > limits.max_total_bytes {
            return Err(GitImportError::LimitExceeded);
        }
        destination
            .write_all(&buffer[..read])
            .map_err(|_| GitImportError::Io)?;
    }
    *counters.object_files = counters
        .object_files
        .checked_add(1)
        .ok_or(GitImportError::LimitExceeded)?;
    Ok(())
}

fn is_hex(bytes: &[u8], len: usize) -> bool {
    bytes.len() == len && bytes.iter().all(|byte| byte.is_ascii_hexdigit())
}

fn is_pack_file(name: &str, hash_len: usize) -> bool {
    let Some(rest) = name.strip_prefix("pack-") else {
        return false;
    };
    let Some((hash, extension)) = rest.rsplit_once('.') else {
        return false;
    };
    is_hex(hash.as_bytes(), hash_len) && matches!(extension, "pack" | "idx" | "rev" | "bitmap")
}

struct MetadataBudget {
    used: u64,
    limit: u64,
}

impl MetadataBudget {
    fn new(limit: u64) -> Self {
        Self { used: 0, limit }
    }

    fn read(&mut self, file: &File) -> Result<Vec<u8>, GitImportError> {
        let remaining = self.limit.saturating_sub(self.used);
        let bytes = read_bounded(file, remaining)?;
        self.used = self
            .used
            .checked_add(bytes.len() as u64)
            .ok_or(GitImportError::LimitExceeded)?;
        Ok(bytes)
    }
}

fn read_bounded(file: &File, limit: u64) -> Result<Vec<u8>, GitImportError> {
    let mut read = file;
    let mut out = Vec::new();
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        let remaining = limit.saturating_sub(out.len() as u64);
        let want = remaining.saturating_add(1).min(COPY_BUFFER_BYTES as u64) as usize;
        let read_bytes = read
            .read(&mut buffer[..want])
            .map_err(|_| GitImportError::Io)?;
        if read_bytes == 0 {
            break;
        }
        if (out.len() as u64).saturating_add(read_bytes as u64) > limit {
            return Err(GitImportError::LimitExceeded);
        }
        out.extend_from_slice(&buffer[..read_bytes]);
    }
    Ok(out)
}

struct HeadCapture {
    raw_head: Vec<u8>,
    raw_ref: Option<Vec<u8>>,
    captured_head: String,
}

fn read_head_capture(
    git: &OwnedFd,
    branch: &str,
    branch_components: &[String],
    expected_head: &str,
    hash_len: usize,
    budget: &mut MetadataBudget,
) -> Result<HeadCapture, GitImportError> {
    let head_file = open_candidate(git, "HEAD")?;
    let raw_head = budget.read(&head_file)?;
    let head_text =
        std::str::from_utf8(&raw_head).map_err(|_| GitImportError::UnsupportedLayout)?;
    let head_text = head_text.strip_suffix('\n').unwrap_or(head_text);
    if head_text.contains('\n') || head_text.contains('\r') {
        return Err(GitImportError::UnsupportedLayout);
    }

    if let Some(target) = head_text.strip_prefix("ref: ") {
        let expected_ref = format!("refs/heads/{branch}");
        if target != expected_ref {
            return Err(GitImportError::HeadMismatch);
        }
        let ref_file = open_ref_file(git, branch_components)?;
        let raw_ref = budget.read(&ref_file)?;
        let ref_text =
            std::str::from_utf8(&raw_ref).map_err(|_| GitImportError::UnsupportedLayout)?;
        let ref_text = ref_text.strip_suffix('\n').unwrap_or(ref_text);
        if ref_text.contains('\n') || ref_text.contains('\r') {
            return Err(GitImportError::UnsupportedLayout);
        }
        if !is_hex(ref_text.as_bytes(), hash_len) {
            return Err(GitImportError::UnsupportedLayout);
        }
        if !ref_text.eq_ignore_ascii_case(expected_head) {
            return Err(GitImportError::HeadMismatch);
        }
        let captured_head = ref_text.to_ascii_lowercase();
        return Ok(HeadCapture {
            raw_head,
            raw_ref: Some(raw_ref),
            captured_head,
        });
    }

    if !is_hex(head_text.as_bytes(), hash_len) {
        return Err(GitImportError::UnsupportedLayout);
    }
    if !head_text.eq_ignore_ascii_case(expected_head) {
        return Err(GitImportError::HeadMismatch);
    }
    let captured_head = head_text.to_ascii_lowercase();
    Ok(HeadCapture {
        raw_head,
        raw_ref: None,
        captured_head,
    })
}

fn open_ref_file(git: &OwnedFd, components: &[String]) -> Result<File, GitImportError> {
    let refs = open_expected_dir(git, "refs")?;
    let mut current = open_expected_dir(&refs, "heads")?;
    for (index, component) in components.iter().enumerate() {
        if index + 1 == components.len() {
            return open_candidate(&current, component);
        }
        current = open_expected_dir(&current, component)?;
    }
    Err(GitImportError::UnsupportedLayout)
}

fn allocate_snapshot(
    parent: &OwnedFd,
    name_override: Option<String>,
) -> Result<(String, OwnedFd), GitImportError> {
    for attempt in 0..64_u32 {
        let name = name_override
            .clone()
            .unwrap_or_else(|| generated_snapshot_name(attempt));
        match mkdirat(parent, &name, Mode::from_bits_truncate(PRIVATE_DIR_MODE)) {
            Ok(()) => {
                let fd = open_expected_dir(parent, &name)
                    .map_err(|_| GitImportError::UnsafeDestination)?;
                fchmod(&fd, Mode::from_bits_truncate(PRIVATE_DIR_MODE))
                    .map_err(|_| GitImportError::Io)?;
                return Ok((name, fd));
            }
            Err(Errno::EXIST) => {
                if name_override.is_some() {
                    return Err(GitImportError::UnsafeDestination);
                }
            }
            Err(_) => return Err(GitImportError::Io),
        }
    }
    Err(GitImportError::Io)
}

fn generated_snapshot_name(attempt: u32) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        "raw-git-{}-{nanos:x}-{counter:x}-{attempt}",
        std::process::id()
    )
}

struct PendingSnapshot<'a> {
    parent: &'a OwnedFd,
    name: String,
    dir: OwnedFd,
    committed: bool,
}

impl Drop for PendingSnapshot<'_> {
    fn drop(&mut self) {
        if !self.committed {
            remove_tree(self.parent, &self.name);
        }
    }
}

fn remove_tree(parent: &OwnedFd, name: &str) {
    if let Ok(dir) = openat(parent, name, DIRECTORY_OPEN_FLAGS, Mode::empty()) {
        remove_tree_contents(&dir);
    }
    let _ = unlinkat(parent, name, AtFlags::REMOVEDIR);
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
pub(super) mod seam {
    use std::cell::RefCell;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Step {
        OpenGitDir,
        SnapshotAllocate,
        OpenObjectDirectory,
        OpenObjectFile,
        CopyObjectFile,
        RecheckHead,
    }

    type Hook = Box<dyn FnMut(Step)>;

    struct State {
        hook: Option<Hook>,
        snapshot_name: Option<String>,
    }

    thread_local! {
        static STATE: RefCell<State> = const {
            RefCell::new(State { hook: None, snapshot_name: None })
        };
    }

    pub(crate) struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.hook = None;
                state.snapshot_name = None;
            });
        }
    }

    pub(crate) fn install(hook: impl FnMut(Step) + 'static) -> Guard {
        STATE.with(|state| state.borrow_mut().hook = Some(Box::new(hook)));
        Guard
    }

    pub(crate) fn snapshot_name(name: &str) -> Guard {
        STATE.with(|state| state.borrow_mut().snapshot_name = Some(name.to_owned()));
        Guard
    }

    pub(crate) fn snapshot_name_override() -> Option<String> {
        STATE.with(|state| state.borrow().snapshot_name.clone())
    }

    pub(crate) fn fire(step: Step) {
        STATE.with(|state| {
            if let Some(hook) = state.borrow_mut().hook.as_mut() {
                hook(step);
            }
        });
    }
}
