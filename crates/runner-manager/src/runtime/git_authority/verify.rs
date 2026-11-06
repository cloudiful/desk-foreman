//! Trusted orchestration of the secret-free Git verification sequence.
//!
//! The raw snapshot is retained unchanged. Verification prepares one fresh
//! private operation directory, transfers only bounded raw object bytes into a
//! new bare repository, and asks a supplied [`TrustedGitRunner`] to execute a
//! closed command sequence. There is no production host runner in this phase.

use std::fs::File;
use std::future::Future;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use runner_protocol::{GitObjectFormat, GitVerifyRequest, GitVerifyResult};
use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags, fchmod, fstat, mkdirat, openat, unlinkat};
use rustix::io::Errno;

use super::artifact::{VerifiedGitIdentity, VerifiedGitRepository};
use super::helper::{GitVerifyLimits, HelperPaths, HelperPlan, plan_command_with_output_limit};
use super::import::{RawGitSnapshot, is_regular_candidate};

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
const MAX_SNAPSHOT_METADATA_BYTES: u64 = 8192;
const MAX_METADATA_ENTRIES: u64 = 64;
const COPY_BUFFER_BYTES: usize = 64 * 1024;

/// Closed command set. No arbitrary arguments, refs, or shell text cross this
/// boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TrustedGitCommand {
    Init {
        object_format: GitObjectFormat,
        initial_branch: String,
    },
    ShowObjectFormat,
    Fsck,
    CatFileCommit {
        head: String,
    },
    MergeBaseIsAncestor {
        base: String,
        head: String,
    },
    RevParseCommit {
        head: String,
    },
    ShowRef {
        branch: String,
    },
    BundleCreate {
        branch: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TrustedGitOutput {
    pub(crate) exit_code: i32,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TrustedGitRunnerError {
    TimedOut,
    Cancelled,
    OutputLimit,
    ReapTimeout,
    Cleanup,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TrustedGitInvocation {
    pub(crate) command: TrustedGitCommand,
    pub(crate) paths: HelperPaths,
    pub(crate) plan: HelperPlan,
}

/// Injectable trusted executor. The Docker helper owns this trait; the
/// production manager has no host implementation.
pub(crate) type TrustedGitFuture<'a> =
    Pin<Box<dyn Future<Output = Result<TrustedGitOutput, TrustedGitRunnerError>> + Send + 'a>>;

pub(crate) trait TrustedGitRunner {
    fn run<'a>(&'a mut self, invocation: TrustedGitInvocation) -> TrustedGitFuture<'a>;
}

/// Fixed, non-secret verification failures. No Git output or host path leaks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GitVerifyError {
    InvalidRequest,
    SnapshotMismatch,
    SnapshotLayout,
    Preparation,
    HelperPlan,
    Timeout,
    OutputLimit,
    NonFastForward,
    ObjectFormatMismatch,
    FsckFailed,
    MissingCommit,
    AncestryFailed,
    HeadMismatch,
    RunnerFailed,
    Cancelled,
    ReapTimeout,
    Cleanup,
}

impl std::fmt::Display for GitVerifyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::InvalidRequest => "git verification request is invalid",
            Self::SnapshotMismatch => "raw git snapshot does not match the request",
            Self::SnapshotLayout => "raw git snapshot layout is unsupported",
            Self::Preparation => "trusted git preparation failed",
            Self::HelperPlan => "trusted git helper plan is invalid",
            Self::Timeout => "git verification timed out",
            Self::OutputLimit => "git verification output exceeded its limit",
            Self::NonFastForward => "git verification ancestry is not fast-forward",
            Self::ObjectFormatMismatch => "git object format does not match the request",
            Self::FsckFailed => "git fsck failed",
            Self::MissingCommit => "expected commit is missing",
            Self::AncestryFailed => "git ancestry check failed",
            Self::HeadMismatch => "git rev-parse does not match the expected head",
            Self::RunnerFailed => "trusted git runner failed",
            Self::Cancelled => "git verification was cancelled",
            Self::ReapTimeout => "trusted git helper reap timed out",
            Self::Cleanup => "trusted git helper cleanup was not confirmed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for GitVerifyError {}

/// Verify one raw DFI1a snapshot. The snapshot is retained; only a new private
/// operation directory is created and removed.
pub(crate) async fn verify_raw_git_snapshot<R: TrustedGitRunner>(
    quarantine_root: &OwnedFd,
    raw_snapshot: &RawGitSnapshot,
    request: &GitVerifyRequest,
    limits: &GitVerifyLimits,
    runner: &mut R,
) -> Result<VerifiedGitRepository, GitVerifyError> {
    request
        .validate()
        .map_err(|_| GitVerifyError::InvalidRequest)?;
    let identity = VerifiedGitIdentity::from_validated_request(request)?;
    if request.snapshot_name != raw_snapshot.snapshot_name
        || request.branch != raw_snapshot.pinned_branch
        || request.expected_new_head != raw_snapshot.captured_head
        || request.object_format.hash_hex_len() != raw_snapshot.captured_head.len()
    {
        return Err(GitVerifyError::SnapshotMismatch);
    }

    let snapshot = open_expected_dir(quarantine_root, request.snapshot_name.as_str())
        .map_err(|_| GitVerifyError::SnapshotLayout)?;
    validate_snapshot_metadata(&snapshot, request)?;

    let operation = Operation::create(quarantine_root)?;
    let mut guard = match OperationGuard::new(quarantine_root, &operation.name) {
        Ok(guard) => guard,
        Err(error) => {
            remove_tree(quarantine_root, &operation.name);
            return Err(error);
        }
    };
    let paths = operation.paths.clone();
    let started = Instant::now();
    let mut output_bytes = 0_usize;

    let init = run_command(
        runner,
        &paths,
        TrustedGitCommand::Init {
            object_format: request.object_format,
            initial_branch: request.branch.clone(),
        },
        limits,
        started,
        &mut output_bytes,
        &mut guard,
    )
    .await?;
    if init.exit_code != 0 {
        return Err(GitVerifyError::Preparation);
    }
    harden_tree(&operation.repo)?;

    copy_snapshot_objects(
        &snapshot,
        &operation.repo,
        raw_snapshot,
        request.object_format.hash_hex_len(),
    )?;

    let format = run_command(
        runner,
        &paths,
        TrustedGitCommand::ShowObjectFormat,
        limits,
        started,
        &mut output_bytes,
        &mut guard,
    )
    .await?;
    if format.exit_code != 0 || trimmed_stdout(&format)? != request.object_format.git_name() {
        return Err(GitVerifyError::ObjectFormatMismatch);
    }

    let fsck = run_command(
        runner,
        &paths,
        TrustedGitCommand::Fsck,
        limits,
        started,
        &mut output_bytes,
        &mut guard,
    )
    .await?;
    if fsck.exit_code != 0 {
        return Err(GitVerifyError::FsckFailed);
    }

    let cat_file = run_command(
        runner,
        &paths,
        TrustedGitCommand::CatFileCommit {
            head: request.expected_new_head.clone(),
        },
        limits,
        started,
        &mut output_bytes,
        &mut guard,
    )
    .await?;
    if cat_file.exit_code != 0 {
        return Err(GitVerifyError::MissingCommit);
    }

    if let Some(base) = &request.expected_base {
        let base_check = run_command(
            runner,
            &paths,
            TrustedGitCommand::CatFileCommit { head: base.clone() },
            limits,
            started,
            &mut output_bytes,
            &mut guard,
        )
        .await?;
        if base_check.exit_code != 0 {
            return Err(GitVerifyError::MissingCommit);
        }
        let ancestor = run_command(
            runner,
            &paths,
            TrustedGitCommand::MergeBaseIsAncestor {
                base: base.clone(),
                head: request.expected_new_head.clone(),
            },
            limits,
            started,
            &mut output_bytes,
            &mut guard,
        )
        .await?;
        match ancestor.exit_code {
            0 => {}
            1 => return Err(GitVerifyError::NonFastForward),
            _ => return Err(GitVerifyError::AncestryFailed),
        }
    }

    let resolved = run_command(
        runner,
        &paths,
        TrustedGitCommand::RevParseCommit {
            head: request.expected_new_head.clone(),
        },
        limits,
        started,
        &mut output_bytes,
        &mut guard,
    )
    .await?;
    if resolved.exit_code != 0 {
        return Err(GitVerifyError::HeadMismatch);
    }
    let resolved = std::str::from_utf8(&resolved.stdout)
        .map(str::trim)
        .map_err(|_| GitVerifyError::HeadMismatch)?;
    if resolved.len() != request.object_format.hash_hex_len()
        || !resolved
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || resolved != request.expected_new_head
    {
        return Err(GitVerifyError::HeadMismatch);
    }

    let summary = GitVerifyResult {
        head: resolved.to_owned(),
        branch: request.branch.clone(),
        base: request.expected_base.clone(),
        object_format: request.object_format,
    };
    Ok(guard.into_artifact(operation, summary, identity))
}

async fn run_command<R: TrustedGitRunner>(
    runner: &mut R,
    paths: &HelperPaths,
    command: TrustedGitCommand,
    limits: &GitVerifyLimits,
    started: Instant,
    output_bytes: &mut usize,
    guard: &mut OperationGuard,
) -> Result<TrustedGitOutput, GitVerifyError> {
    let elapsed = started.elapsed();
    if elapsed >= limits.timeout {
        return Err(GitVerifyError::Timeout);
    }
    let remaining_output = limits.max_output_bytes.saturating_sub(*output_bytes);
    if remaining_output == 0 {
        return Err(GitVerifyError::OutputLimit);
    }
    let plan = plan_command_with_output_limit(
        paths,
        &command,
        limits,
        limits.timeout - elapsed,
        remaining_output,
    )
    .map_err(|_| GitVerifyError::HelperPlan)?;
    guard.mark_nonquiescent();
    let output = match runner
        .run(TrustedGitInvocation {
            command,
            paths: paths.clone(),
            plan,
        })
        .await
    {
        Ok(output) => {
            guard.mark_quiescent();
            output
        }
        Err(error) => {
            let verify_error = match error {
                TrustedGitRunnerError::TimedOut => GitVerifyError::Timeout,
                TrustedGitRunnerError::Cancelled => GitVerifyError::Cancelled,
                TrustedGitRunnerError::OutputLimit => GitVerifyError::OutputLimit,
                TrustedGitRunnerError::ReapTimeout => GitVerifyError::ReapTimeout,
                TrustedGitRunnerError::Cleanup => GitVerifyError::Cleanup,
                TrustedGitRunnerError::Failed => GitVerifyError::RunnerFailed,
            };
            if !matches!(
                verify_error,
                GitVerifyError::ReapTimeout | GitVerifyError::Cleanup
            ) {
                guard.mark_quiescent();
            }
            return Err(verify_error);
        }
    };
    *output_bytes = output_bytes
        .checked_add(output.stdout.len())
        .and_then(|total| total.checked_add(output.stderr.len()))
        .ok_or(GitVerifyError::OutputLimit)?;
    if *output_bytes > limits.max_output_bytes {
        return Err(GitVerifyError::OutputLimit);
    }
    Ok(output)
}

fn trimmed_stdout(output: &TrustedGitOutput) -> Result<&str, GitVerifyError> {
    std::str::from_utf8(&output.stdout)
        .map(str::trim)
        .map_err(|_| GitVerifyError::ObjectFormatMismatch)
}
fn validate_snapshot_metadata(
    snapshot: &OwnedFd,
    request: &GitVerifyRequest,
) -> Result<(), GitVerifyError> {
    let head_file = open_regular(snapshot, "HEAD")?;
    let head = read_bounded(&head_file, MAX_SNAPSHOT_METADATA_BYTES)?;
    let head = metadata_text(&head)?;

    let mut entries = list_entries(snapshot, MAX_METADATA_ENTRIES)?;
    entries.sort();
    if entries != ["HEAD".to_owned(), "objects".to_owned()]
        && entries != ["HEAD".to_owned(), "objects".to_owned(), "refs".to_owned()]
    {
        return Err(GitVerifyError::SnapshotLayout);
    }
    open_expected_dir(snapshot, "objects").map_err(|_| GitVerifyError::SnapshotLayout)?;

    if let Some(target) = head.strip_prefix("ref: ") {
        let expected_ref = format!("refs/heads/{}", request.branch);
        if target != expected_ref {
            return Err(GitVerifyError::HeadMismatch);
        }
        if !entries.iter().any(|entry| entry == "refs") {
            return Err(GitVerifyError::SnapshotLayout);
        }
        let refs =
            open_expected_dir(snapshot, "refs").map_err(|_| GitVerifyError::SnapshotLayout)?;
        let ref_file = open_ref_file(&refs, &request.branch)?;
        let value = read_bounded(&ref_file, MAX_SNAPSHOT_METADATA_BYTES)?;
        let value = metadata_text(&value)?;
        if value != request.expected_new_head {
            return Err(GitVerifyError::HeadMismatch);
        }
        validate_refs_tree(&refs, &request.branch)?;
    } else {
        if head != request.expected_new_head {
            return Err(GitVerifyError::HeadMismatch);
        }
        if entries.iter().any(|entry| entry == "refs") {
            return Err(GitVerifyError::SnapshotLayout);
        }
    }
    Ok(())
}

fn validate_refs_tree(refs: &OwnedFd, branch: &str) -> Result<(), GitVerifyError> {
    let entries = list_entries(refs, MAX_METADATA_ENTRIES)?;
    if entries != ["heads".to_owned()] {
        return Err(GitVerifyError::SnapshotLayout);
    }
    let components: Vec<&str> = branch.split('/').collect();
    let mut current =
        open_expected_dir(refs, "heads").map_err(|_| GitVerifyError::SnapshotLayout)?;
    for (index, component) in components.iter().enumerate() {
        let entries = list_entries(&current, MAX_METADATA_ENTRIES)?;
        if entries != [(*component).to_owned()] {
            return Err(GitVerifyError::SnapshotLayout);
        }
        if index + 1 == components.len() {
            open_regular(&current, component)?;
        } else {
            current = open_expected_dir(&current, *component)
                .map_err(|_| GitVerifyError::SnapshotLayout)?;
        }
    }
    Ok(())
}

fn open_ref_file(refs: &OwnedFd, branch: &str) -> Result<File, GitVerifyError> {
    let components: Vec<&str> = branch.split('/').collect();
    let mut current =
        open_expected_dir(refs, "heads").map_err(|_| GitVerifyError::SnapshotLayout)?;
    for (index, component) in components.iter().enumerate() {
        if index + 1 == components.len() {
            return open_regular(&current, component);
        }
        current =
            open_expected_dir(&current, *component).map_err(|_| GitVerifyError::SnapshotLayout)?;
    }
    Err(GitVerifyError::SnapshotLayout)
}

fn metadata_text(bytes: &[u8]) -> Result<&str, GitVerifyError> {
    let text = std::str::from_utf8(bytes).map_err(|_| GitVerifyError::SnapshotLayout)?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    if text.contains('\n') || text.contains('\r') {
        return Err(GitVerifyError::SnapshotLayout);
    }
    Ok(text)
}

struct CopyCounters {
    files: u64,
    bytes: u64,
    entries: u64,
}

fn copy_snapshot_objects(
    snapshot: &OwnedFd,
    repo: &OwnedFd,
    raw_snapshot: &RawGitSnapshot,
    hash_len: usize,
) -> Result<(), GitVerifyError> {
    let source =
        open_expected_dir(snapshot, "objects").map_err(|_| GitVerifyError::SnapshotLayout)?;
    let destination =
        open_expected_dir(repo, "objects").map_err(|_| GitVerifyError::Preparation)?;
    let max_entries = raw_snapshot
        .object_files
        .saturating_mul(4)
        .saturating_add(4096);
    let mut counters = CopyCounters {
        files: 0,
        bytes: 0,
        entries: 0,
    };
    let mut reader = Dir::read_from(&source).map_err(|_| GitVerifyError::SnapshotLayout)?;
    while let Some(entry) = reader.read() {
        let entry = entry.map_err(|_| GitVerifyError::SnapshotLayout)?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        bump_entries(&mut counters, max_entries)?;
        let name = std::str::from_utf8(name).map_err(|_| GitVerifyError::SnapshotLayout)?;
        if name == "pack" {
            let src =
                open_expected_dir(&source, "pack").map_err(|_| GitVerifyError::SnapshotLayout)?;
            let dst = open_or_create_dir(&destination, "pack")?;
            copy_pack_dir(&src, &dst, raw_snapshot, hash_len, &mut counters)?;
        } else if is_hex(name.as_bytes(), 2) {
            let src =
                open_expected_dir(&source, name).map_err(|_| GitVerifyError::SnapshotLayout)?;
            let dst = open_or_create_dir(&destination, name)?;
            copy_loose_dir(&src, &dst, raw_snapshot, hash_len, &mut counters)?;
        } else {
            return Err(GitVerifyError::SnapshotLayout);
        }
    }
    if counters.files != raw_snapshot.object_files || counters.bytes != raw_snapshot.object_bytes {
        return Err(GitVerifyError::SnapshotMismatch);
    }
    Ok(())
}

fn copy_pack_dir(
    source: &OwnedFd,
    destination: &OwnedFd,
    raw_snapshot: &RawGitSnapshot,
    hash_len: usize,
    counters: &mut CopyCounters,
) -> Result<(), GitVerifyError> {
    let mut reader = Dir::read_from(source).map_err(|_| GitVerifyError::SnapshotLayout)?;
    while let Some(entry) = reader.read() {
        let entry = entry.map_err(|_| GitVerifyError::SnapshotLayout)?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        bump_entries(
            counters,
            raw_snapshot
                .object_files
                .saturating_mul(4)
                .saturating_add(4096),
        )?;
        let name = std::str::from_utf8(name).map_err(|_| GitVerifyError::SnapshotLayout)?;
        if !is_pack_file(name, hash_len) {
            return Err(GitVerifyError::SnapshotLayout);
        }
        copy_regular_file(source, name, destination, name, raw_snapshot, counters)?;
    }
    Ok(())
}

fn copy_loose_dir(
    source: &OwnedFd,
    destination: &OwnedFd,
    raw_snapshot: &RawGitSnapshot,
    hash_len: usize,
    counters: &mut CopyCounters,
) -> Result<(), GitVerifyError> {
    let mut reader = Dir::read_from(source).map_err(|_| GitVerifyError::SnapshotLayout)?;
    while let Some(entry) = reader.read() {
        let entry = entry.map_err(|_| GitVerifyError::SnapshotLayout)?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        bump_entries(
            counters,
            raw_snapshot
                .object_files
                .saturating_mul(4)
                .saturating_add(4096),
        )?;
        let name = std::str::from_utf8(name).map_err(|_| GitVerifyError::SnapshotLayout)?;
        if !is_hex(name.as_bytes(), hash_len.saturating_sub(2)) {
            return Err(GitVerifyError::SnapshotLayout);
        }
        copy_regular_file(source, name, destination, name, raw_snapshot, counters)?;
    }
    Ok(())
}

fn copy_regular_file(
    source_parent: &OwnedFd,
    source_name: &str,
    destination_parent: &OwnedFd,
    destination_name: &str,
    raw_snapshot: &RawGitSnapshot,
    counters: &mut CopyCounters,
) -> Result<(), GitVerifyError> {
    let mut source = open_candidate(source_parent, source_name)?;
    counters.files = counters
        .files
        .checked_add(1)
        .ok_or(GitVerifyError::SnapshotMismatch)?;
    if counters.files > raw_snapshot.object_files {
        return Err(GitVerifyError::SnapshotMismatch);
    }
    let mut destination = create_output_file(destination_parent, destination_name)?;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        let remaining = raw_snapshot.object_bytes.saturating_sub(counters.bytes);
        let want = remaining.saturating_add(1).min(COPY_BUFFER_BYTES as u64) as usize;
        let read = source
            .read(&mut buffer[..want])
            .map_err(|_| GitVerifyError::Preparation)?;
        if read == 0 {
            break;
        }
        counters.bytes = counters
            .bytes
            .checked_add(read as u64)
            .ok_or(GitVerifyError::SnapshotMismatch)?;
        if counters.bytes > raw_snapshot.object_bytes {
            return Err(GitVerifyError::SnapshotMismatch);
        }
        destination
            .write_all(&buffer[..read])
            .map_err(|_| GitVerifyError::Preparation)?;
    }
    Ok(())
}

fn bump_entries(counters: &mut CopyCounters, max_entries: u64) -> Result<(), GitVerifyError> {
    counters.entries = counters
        .entries
        .checked_add(1)
        .ok_or(GitVerifyError::SnapshotLayout)?;
    if counters.entries > max_entries {
        return Err(GitVerifyError::SnapshotLayout);
    }
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

fn list_entries(parent: &OwnedFd, max_entries: u64) -> Result<Vec<String>, GitVerifyError> {
    let mut reader = Dir::read_from(parent).map_err(|_| GitVerifyError::SnapshotLayout)?;
    let mut entries = Vec::new();
    while let Some(entry) = reader.read() {
        let entry = entry.map_err(|_| GitVerifyError::SnapshotLayout)?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        entries.push(
            std::str::from_utf8(name)
                .map_err(|_| GitVerifyError::SnapshotLayout)?
                .to_owned(),
        );
        if entries.len() as u64 > max_entries {
            return Err(GitVerifyError::SnapshotLayout);
        }
    }
    Ok(entries)
}

fn open_regular(parent: &OwnedFd, name: &str) -> Result<File, GitVerifyError> {
    open_candidate(parent, name)
}

fn open_candidate(parent: &OwnedFd, name: &str) -> Result<File, GitVerifyError> {
    let fd = openat(parent, name, CANDIDATE_OPEN_FLAGS, Mode::empty())
        .map_err(|_| GitVerifyError::SnapshotLayout)?;
    let stat = fstat(&fd).map_err(|_| GitVerifyError::Preparation)?;
    if !is_regular_candidate(FileType::from_raw_mode(stat.st_mode), stat.st_nlink) {
        return Err(GitVerifyError::SnapshotLayout);
    }
    Ok(File::from(fd))
}

fn open_expected_dir<P: rustix::path::Arg>(
    parent: &OwnedFd,
    name: P,
) -> Result<OwnedFd, GitVerifyError> {
    let fd = openat(parent, name, DIRECTORY_OPEN_FLAGS, Mode::empty())
        .map_err(|_| GitVerifyError::SnapshotLayout)?;
    let stat = fstat(&fd).map_err(|_| GitVerifyError::Preparation)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
        return Err(GitVerifyError::SnapshotLayout);
    }
    Ok(fd)
}

fn harden_tree(directory: &OwnedFd) -> Result<(), GitVerifyError> {
    let mut reader = Dir::read_from(directory).map_err(|_| GitVerifyError::Preparation)?;
    while let Some(entry) = reader.read() {
        let entry = entry.map_err(|_| GitVerifyError::Preparation)?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        let stat = rustix::fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_| GitVerifyError::Preparation)?;
        match FileType::from_raw_mode(stat.st_mode) {
            FileType::Directory => {
                let child =
                    open_expected_dir(directory, name).map_err(|_| GitVerifyError::Preparation)?;
                harden_tree(&child)?;
                fchmod(&child, Mode::from_bits_truncate(PRIVATE_DIR_MODE))
                    .map_err(|_| GitVerifyError::Preparation)?;
            }
            FileType::RegularFile => {
                let file = openat(directory, name, CANDIDATE_OPEN_FLAGS, Mode::empty())
                    .map_err(|_| GitVerifyError::Preparation)?;
                fchmod(&file, Mode::from_bits_truncate(PRIVATE_FILE_MODE))
                    .map_err(|_| GitVerifyError::Preparation)?;
            }
            _ => return Err(GitVerifyError::Preparation),
        }
    }
    fchmod(directory, Mode::from_bits_truncate(PRIVATE_DIR_MODE))
        .map_err(|_| GitVerifyError::Preparation)?;
    Ok(())
}

fn open_or_create_dir(parent: &OwnedFd, name: &str) -> Result<OwnedFd, GitVerifyError> {
    if mkdirat(parent, name, Mode::from_bits_truncate(PRIVATE_DIR_MODE)).is_err() {
        // An existing directory is fine; the no-follow open below decides
        // whether the entry is actually a directory.
    }
    let fd = open_expected_dir(parent, name).map_err(|_| GitVerifyError::Preparation)?;
    fchmod(&fd, Mode::from_bits_truncate(PRIVATE_DIR_MODE))
        .map_err(|_| GitVerifyError::Preparation)?;
    Ok(fd)
}

fn create_dir(parent: &OwnedFd, name: &str) -> Result<OwnedFd, GitVerifyError> {
    mkdirat(parent, name, Mode::from_bits_truncate(PRIVATE_DIR_MODE))
        .map_err(|_| GitVerifyError::Preparation)?;
    let fd = open_expected_dir(parent, name).map_err(|_| GitVerifyError::Preparation)?;
    fchmod(&fd, Mode::from_bits_truncate(PRIVATE_DIR_MODE))
        .map_err(|_| GitVerifyError::Preparation)?;
    Ok(fd)
}

fn create_output_file(parent: &OwnedFd, name: &str) -> Result<File, GitVerifyError> {
    let fd = openat(
        parent,
        name,
        OUTPUT_FILE_FLAGS,
        Mode::from_bits_truncate(PRIVATE_FILE_MODE),
    )
    .map_err(|_| GitVerifyError::Preparation)?;
    fchmod(&fd, Mode::from_bits_truncate(PRIVATE_FILE_MODE))
        .map_err(|_| GitVerifyError::Preparation)?;
    Ok(File::from(fd))
}

fn read_bounded(file: &File, limit: u64) -> Result<Vec<u8>, GitVerifyError> {
    let mut read = file;
    let mut out = Vec::new();
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        let remaining = limit.saturating_sub(out.len() as u64);
        let want = remaining.saturating_add(1).min(COPY_BUFFER_BYTES as u64) as usize;
        let read_bytes = read
            .read(&mut buffer[..want])
            .map_err(|_| GitVerifyError::Preparation)?;
        if read_bytes == 0 {
            break;
        }
        if (out.len() as u64).saturating_add(read_bytes as u64) > limit {
            return Err(GitVerifyError::SnapshotLayout);
        }
        out.extend_from_slice(&buffer[..read_bytes]);
    }
    Ok(out)
}

struct Operation {
    name: String,
    repo: OwnedFd,
    paths: HelperPaths,
}

impl Operation {
    fn create(parent: &OwnedFd) -> Result<Self, GitVerifyError> {
        let parent_path = fd_path(parent)?;
        let mut selected = None;
        for attempt in 0..64_u32 {
            let name = generated_operation_name(attempt);
            match mkdirat(parent, &name, Mode::from_bits_truncate(PRIVATE_DIR_MODE)) {
                Ok(()) => {
                    selected = Some(name);
                    break;
                }
                Err(Errno::EXIST) => continue,
                Err(_) => return Err(GitVerifyError::Preparation),
            }
        }
        let name = selected.ok_or(GitVerifyError::Preparation)?;
        let result = (|| -> Result<Self, GitVerifyError> {
            let dir = open_expected_dir(parent, &name).map_err(|_| GitVerifyError::Preparation)?;
            fchmod(&dir, Mode::from_bits_truncate(PRIVATE_DIR_MODE))
                .map_err(|_| GitVerifyError::Preparation)?;
            let repo = create_dir(&dir, "repo")?;
            create_dir(&dir, "home")?;
            create_dir(&dir, "template")?;
            let stat = fstat(&dir).map_err(|_| GitVerifyError::Preparation)?;
            let root = parent_path.join(&name);
            Ok(Self {
                name: name.clone(),
                repo,
                paths: HelperPaths {
                    operation_root: root.clone(),
                    repo_dir: root.join("repo"),
                    home_dir: root.join("home"),
                    template_dir: root.join("template"),
                    uid: stat.st_uid,
                    gid: stat.st_gid,
                },
            })
        })();
        if result.is_err() {
            remove_tree(parent, &name);
        }
        result
    }
}

fn fd_path(fd: &OwnedFd) -> Result<PathBuf, GitVerifyError> {
    let link = format!("/proc/self/fd/{}", fd.as_raw_fd());
    let path = std::fs::read_link(link).map_err(|_| GitVerifyError::Preparation)?;
    if !path.is_absolute() || path.to_string_lossy().contains(" (deleted)") {
        return Err(GitVerifyError::Preparation);
    }
    Ok(path)
}

fn generated_operation_name(attempt: u32) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        "git-verify-{}-{nanos:x}-{counter:x}-{attempt}",
        std::process::id()
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Quiescence {
    Quiescent,
    NonQuiescent,
    Transferred,
}

struct OperationGuard {
    parent: Option<OwnedFd>,
    name: Option<String>,
    state: Quiescence,
}

impl OperationGuard {
    fn new(parent: &OwnedFd, name: &str) -> Result<Self, GitVerifyError> {
        Ok(Self {
            parent: Some(
                parent
                    .try_clone()
                    .map_err(|_| GitVerifyError::Preparation)?,
            ),
            name: Some(name.to_owned()),
            state: Quiescence::Quiescent,
        })
    }

    fn mark_nonquiescent(&mut self) {
        self.state = Quiescence::NonQuiescent;
    }

    fn mark_quiescent(&mut self) {
        if self.state == Quiescence::NonQuiescent {
            self.state = Quiescence::Quiescent;
        }
    }

    fn into_artifact(
        mut self,
        operation: Operation,
        summary: GitVerifyResult,
        identity: VerifiedGitIdentity,
    ) -> VerifiedGitRepository {
        self.state = Quiescence::Transferred;
        VerifiedGitRepository::new(
            self.parent.take().expect("operation parent"),
            self.name.take().expect("operation name"),
            operation.repo,
            summary,
            identity,
            operation.paths,
        )
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        if self.state == Quiescence::Quiescent
            && let (Some(parent), Some(name)) = (&self.parent, &self.name)
        {
            remove_tree(parent, name);
        }
    }
}

pub(super) fn remove_tree(parent: &OwnedFd, name: &str) {
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
            FileType::Unknown => rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)
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

/// Validate a fetched repository's local branch ref against the pinned head.
/// This reads bounded helper metadata only and never imports objects.
pub(crate) fn verify_fetched_head(
    show_ref_stdout: &[u8],
    branch: &str,
    pinned_head: &str,
) -> Result<(), GitVerifyError> {
    super::clone_fetch::parse_local_ref(show_ref_stdout, branch, pinned_head)
        .map_err(|_| GitVerifyError::HeadMismatch)
}
