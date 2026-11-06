//! Credential-free staging of a verified private repository into one bundle.
//!
//! Staging consumes the verified artifact by value. It never recopies the raw
//! snapshot, initializes a second repository, or exposes a public path. The
//! bundle is produced by the closed helper command set and written once by the
//! manager into an exclusive private file.

use std::fmt;
use std::fs::File;
use std::io::{Read, Write};
use std::num::NonZeroUsize;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use runner_protocol::{GitVerifyRequest, GitVerifyResult};
use rustix::fs::{
    AtFlags, Dir, FileType, Mode, OFlags, fchmod, fstat, fsync, mkdirat, openat, statat,
};
use rustix::io::Errno;

use super::artifact::{VerifiedGitIdentity, VerifiedGitRepository};
use super::helper::{
    GitVerifyLimits, HelperPaths, is_valid_branch, plan_command_with_output_limit,
};
use super::verify::{
    TrustedGitCommand, TrustedGitInvocation, TrustedGitOutput, TrustedGitRunner,
    TrustedGitRunnerError, remove_tree,
};

pub(crate) const MAX_EXPORT_BUNDLE_BYTES: usize = 64 * 1024 * 1024;
const PRIVATE_DIR_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;
const BUNDLE_FILE_NAME: &str = "bundle.gitbundle";

const DIRECTORY_OPEN_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const BUNDLE_OPEN_FLAGS: OFlags = OFlags::WRONLY
    .union(OFlags::CREATE)
    .union(OFlags::EXCL)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct GitExportLimit {
    max_bundle_bytes: NonZeroUsize,
}

impl GitExportLimit {
    pub(crate) fn new(max_bundle_bytes: usize) -> Result<Self, GitStageError> {
        let max_bundle_bytes =
            NonZeroUsize::new(max_bundle_bytes).ok_or(GitStageError::InvalidExportLimit)?;
        if max_bundle_bytes.get() > MAX_EXPORT_BUNDLE_BYTES {
            return Err(GitStageError::ExportLimitTooLarge);
        }
        Ok(Self { max_bundle_bytes })
    }

    pub(crate) fn max_bundle_bytes(self) -> usize {
        self.max_bundle_bytes.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GitStageError {
    IdentityMismatch,
    InvalidRequest,
    AlternatesPresent,
    InvalidExportLimit,
    ExportLimitTooLarge,
    HelperPlan,
    Preparation,
    Staging,
    RefExists,
    RefFormatUnsupported,
    RefWriteFailed,
    RefVerifyFailed,
    IncompleteHistory,
    OutputLimit,
    ExportTooLarge,
    Timeout,
    Cancelled,
    ReapTimeout,
    CleanupUnconfirmed,
    ExportWrite,
    RunnerFailed,
}

impl fmt::Display for GitStageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::IdentityMismatch => "verified git identity does not match the request",
            Self::InvalidRequest => "git staging request is invalid",
            Self::AlternatesPresent => "verified git repository has alternates",
            Self::InvalidExportLimit => "git export limit must be positive",
            Self::ExportLimitTooLarge => "git export limit exceeds the ceiling",
            Self::HelperPlan => "git export helper plan is invalid",
            Self::Preparation => "git export preparation failed",
            Self::Staging => "git export staging directory is unavailable",
            Self::RefExists => "git export ref already exists",
            Self::RefFormatUnsupported => "git export ref format is unsupported",
            Self::RefWriteFailed => "git export ref write failed",
            Self::RefVerifyFailed => "git export ref verification failed",
            Self::IncompleteHistory => "git export history is incomplete",
            Self::OutputLimit => "git export output exceeded its limit",
            Self::ExportTooLarge => "git export bundle exceeds its limit",
            Self::Timeout => "git export timed out",
            Self::Cancelled => "git export was cancelled",
            Self::ReapTimeout => "git export helper reap timed out",
            Self::CleanupUnconfirmed => "git export helper cleanup was not confirmed",
            Self::ExportWrite => "git export bundle write failed",
            Self::RunnerFailed => "trusted git export runner failed",
        })
    }
}

impl std::error::Error for GitStageError {}

pub(crate) struct StagedGitRepository {
    verified: VerifiedGitRepository,
    parent: OwnedFd,
    name: String,
    bundle: File,
}

impl StagedGitRepository {
    pub(crate) fn summary(&self) -> &GitVerifyResult {
        self.verified.summary()
    }

    pub(crate) fn identity(&self) -> &VerifiedGitIdentity {
        self.verified.identity()
    }

    #[cfg(test)]
    pub(super) fn staging_name(&self) -> &str {
        &self.name
    }

    #[cfg(test)]
    pub(super) fn bundle_fd(&self) -> BorrowedFd<'_> {
        self.bundle.as_fd()
    }
}

impl Drop for StagedGitRepository {
    fn drop(&mut self) {
        let _ = self.bundle.as_fd();
        remove_tree(&self.parent, &self.name);
    }
}

impl fmt::Debug for StagedGitRepository {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StagedGitRepository")
            .field("summary", self.summary())
            .field("identity", self.identity())
            .finish_non_exhaustive()
    }
}

pub(crate) async fn stage_verified_bundle<R: TrustedGitRunner>(
    mut verified: VerifiedGitRepository,
    request: &GitVerifyRequest,
    staging_parent: &OwnedFd,
    limits: &GitVerifyLimits,
    export: GitExportLimit,
    runner: &mut R,
) -> Result<StagedGitRepository, GitStageError> {
    request
        .validate()
        .map_err(|_| GitStageError::InvalidRequest)?;
    if !verified.identity().matches(request) {
        return Err(GitStageError::IdentityMismatch);
    }
    let summary = verified.summary();
    if request.expected_new_head != summary.head
        || request.branch != summary.branch
        || request.expected_base != summary.base
        || request.object_format != summary.object_format
    {
        return Err(GitStageError::IdentityMismatch);
    }
    ensure_no_alternates(verified.repo_fd())?;
    write_verified_ref(&verified, &request.branch, &request.expected_new_head)?;

    let deadline = Instant::now()
        .checked_add(limits.timeout)
        .ok_or(GitStageError::Preparation)?;
    let paths = verified.helper_paths().clone();
    let total_limit = export.max_bundle_bytes();
    let mut used = 0_usize;

    let show_cap = output_cap(total_limit, used, limits.max_output_bytes)?;
    let show = run_step(
        &mut verified,
        runner,
        &paths,
        TrustedGitCommand::ShowRef {
            branch: request.branch.clone(),
        },
        limits,
        deadline,
        show_cap,
    )
    .await?;
    if show.exit_code != 0 {
        return Err(GitStageError::RefVerifyFailed);
    }
    let shown = std::str::from_utf8(&show.stdout)
        .map(str::trim)
        .map_err(|_| GitStageError::RefVerifyFailed)?;
    if shown != request.expected_new_head {
        return Err(GitStageError::RefVerifyFailed);
    }
    used = account_output(used, &show, show_cap, total_limit)?;

    let bundle_cap = output_cap(total_limit, used, total_limit)?;
    let bundle = run_step(
        &mut verified,
        runner,
        &paths,
        TrustedGitCommand::BundleCreate {
            branch: request.branch.clone(),
        },
        limits,
        deadline,
        bundle_cap,
    )
    .await?;
    if bundle.exit_code != 0 {
        return Err(GitStageError::IncompleteHistory);
    }
    used = account_output(used, &bundle, bundle_cap, total_limit)?;
    if used > total_limit || bundle.stdout.len() > total_limit {
        return Err(GitStageError::ExportTooLarge);
    }

    let (parent, name) = create_staging_dir(staging_parent)?;
    let guard_parent = match parent.try_clone() {
        Ok(fd) => fd,
        Err(_) => {
            remove_tree(&parent, &name);
            return Err(GitStageError::Staging);
        }
    };
    let mut guard = StagingGuard {
        parent: Some(guard_parent),
        name: name.clone(),
        armed: true,
    };
    let bundle = write_bundle(&parent, &name, &bundle.stdout, paths.uid)?;
    let staged_parent = parent.try_clone().map_err(|_| GitStageError::Staging)?;
    guard.armed = false;
    Ok(StagedGitRepository {
        verified,
        parent: staged_parent,
        name,
        bundle,
    })
}

async fn run_step<R: TrustedGitRunner>(
    verified: &mut VerifiedGitRepository,
    runner: &mut R,
    paths: &HelperPaths,
    command: TrustedGitCommand,
    limits: &GitVerifyLimits,
    deadline: Instant,
    output_limit: usize,
) -> Result<TrustedGitOutput, GitStageError> {
    let now = Instant::now();
    if now >= deadline {
        return Err(GitStageError::Timeout);
    }
    let plan = plan_command_with_output_limit(
        paths,
        &command,
        limits,
        deadline.saturating_duration_since(now),
        output_limit,
    )
    .map_err(|_| GitStageError::HelperPlan)?;
    verified.begin_operation();
    match runner
        .run(TrustedGitInvocation {
            command,
            paths: paths.clone(),
            plan,
        })
        .await
    {
        Ok(output) => {
            verified.confirm_quiescent();
            Ok(output)
        }
        Err(error) => {
            let stage_error = match error {
                TrustedGitRunnerError::TimedOut => GitStageError::Timeout,
                TrustedGitRunnerError::Cancelled => GitStageError::Cancelled,
                TrustedGitRunnerError::OutputLimit => GitStageError::OutputLimit,
                TrustedGitRunnerError::ReapTimeout => GitStageError::ReapTimeout,
                TrustedGitRunnerError::Cleanup => GitStageError::CleanupUnconfirmed,
                TrustedGitRunnerError::Failed => GitStageError::RunnerFailed,
            };
            if !matches!(
                error,
                TrustedGitRunnerError::ReapTimeout | TrustedGitRunnerError::Cleanup
            ) {
                verified.confirm_quiescent();
            }
            Err(stage_error)
        }
    }
}

fn output_cap(total: usize, used: usize, step_max: usize) -> Result<usize, GitStageError> {
    let remaining = total.checked_sub(used).ok_or(GitStageError::OutputLimit)?;
    let cap = remaining.min(step_max);
    if cap == 0 {
        return Err(GitStageError::OutputLimit);
    }
    Ok(cap)
}

fn account_output(
    used: usize,
    output: &TrustedGitOutput,
    step_cap: usize,
    total_limit: usize,
) -> Result<usize, GitStageError> {
    let step = output
        .stdout
        .len()
        .checked_add(output.stderr.len())
        .ok_or(GitStageError::OutputLimit)?;
    if step > step_cap {
        return Err(GitStageError::OutputLimit);
    }
    let total = used.checked_add(step).ok_or(GitStageError::OutputLimit)?;
    if total > total_limit {
        return Err(GitStageError::OutputLimit);
    }
    Ok(total)
}

pub(super) fn write_verified_ref(
    verified: &VerifiedGitRepository,
    branch: &str,
    head: &str,
) -> Result<(), GitStageError> {
    if !is_valid_branch(branch) || (!is_lower_hex_exact(head, 40) && !is_lower_hex_exact(head, 64))
    {
        return Err(GitStageError::RefWriteFailed);
    }
    let uid = verified.helper_paths().uid;
    let repo = verified.repo_fd();
    let head_text = read_trusted_metadata(repo, "HEAD", 1024, uid)?;
    if head_text != format!("ref: refs/heads/{branch}\n") {
        return Err(GitStageError::RefFormatUnsupported);
    }
    let config = read_trusted_metadata(repo, "config", 8192, uid)?;
    let config = config.to_ascii_lowercase();
    if [
        "reftable",
        "refstorage",
        "alternates",
        "include",
        "hookspath",
    ]
    .iter()
    .any(|forbidden| config.contains(forbidden))
    {
        return Err(GitStageError::RefFormatUnsupported);
    }
    for name in ["packed-refs", "reftable"] {
        match statat(repo, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => return Err(GitStageError::RefFormatUnsupported),
            Err(Errno::NOENT) => {}
            Err(_) => return Err(GitStageError::RefFormatUnsupported),
        }
    }
    let refs = openat(repo, "refs", DIRECTORY_OPEN_FLAGS, Mode::empty())
        .map_err(|_| GitStageError::RefFormatUnsupported)?;
    let heads = openat(&refs, "heads", DIRECTORY_OPEN_FLAGS, Mode::empty())
        .map_err(|_| GitStageError::RefFormatUnsupported)?;
    validate_existing_directory(&heads, uid)?;
    let mut reader = Dir::read_from(&heads).map_err(|_| GitStageError::RefFormatUnsupported)?;
    while let Some(entry) = reader.read() {
        let entry = entry.map_err(|_| GitStageError::RefFormatUnsupported)?;
        let name = entry.file_name().to_bytes();
        if name != b"." && name != b".." {
            return Err(GitStageError::RefExists);
        }
    }

    let mut components = branch.split('/');
    let final_name = components
        .next_back()
        .ok_or(GitStageError::RefWriteFailed)?;
    let mut current = heads;
    let mut created = Vec::new();
    for component in components {
        match mkdirat(
            &current,
            component,
            Mode::from_bits_truncate(PRIVATE_DIR_MODE),
        ) {
            Ok(()) => {
                let dir = openat(&current, component, DIRECTORY_OPEN_FLAGS, Mode::empty())
                    .map_err(|_| GitStageError::RefWriteFailed)?;
                fchmod(&dir, Mode::from_bits_truncate(PRIVATE_DIR_MODE))
                    .map_err(|_| GitStageError::RefWriteFailed)?;
                created.push(dir.try_clone().map_err(|_| GitStageError::RefWriteFailed)?);
                current = dir;
            }
            Err(Errno::EXIST) => {
                current = openat(&current, component, DIRECTORY_OPEN_FLAGS, Mode::empty())
                    .map_err(|_| GitStageError::RefFormatUnsupported)?;
                validate_existing_directory(&current, uid)?;
            }
            Err(_) => return Err(GitStageError::RefWriteFailed),
        }
    }

    let fd = openat(
        &current,
        final_name,
        OFlags::WRONLY
            .union(OFlags::CREATE)
            .union(OFlags::EXCL)
            .union(OFlags::NOFOLLOW)
            .union(OFlags::CLOEXEC),
        Mode::from_bits_truncate(PRIVATE_FILE_MODE),
    )
    .map_err(|error| match error {
        Errno::EXIST => GitStageError::RefExists,
        _ => GitStageError::RefWriteFailed,
    })?;
    fchmod(&fd, Mode::from_bits_truncate(PRIVATE_FILE_MODE))
        .map_err(|_| GitStageError::RefWriteFailed)?;
    let mut file = File::from(fd);
    file.write_all(format!("{head}\n").as_bytes())
        .map_err(|_| GitStageError::RefWriteFailed)?;
    file.sync_all().map_err(|_| GitStageError::RefWriteFailed)?;
    let stat = fstat(&file).map_err(|_| GitStageError::RefWriteFailed)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_size < 0
        || stat.st_size as u64 != head.len() as u64 + 1
        || stat.st_uid != uid
        || stat.st_nlink != 1
        || stat.st_mode & 0o7777 != PRIVATE_FILE_MODE
    {
        return Err(GitStageError::RefWriteFailed);
    }
    fsync(&current).map_err(|_| GitStageError::RefWriteFailed)?;
    for dir in &created {
        fsync(dir).map_err(|_| GitStageError::RefWriteFailed)?;
    }
    fsync(&refs).map_err(|_| GitStageError::RefWriteFailed)?;
    fsync(repo).map_err(|_| GitStageError::RefWriteFailed)?;
    Ok(())
}

fn read_trusted_metadata(
    parent: BorrowedFd<'_>,
    name: &str,
    max_bytes: usize,
    uid: u32,
) -> Result<String, GitStageError> {
    let fd = openat(
        parent,
        name,
        OFlags::RDONLY
            .union(OFlags::NOFOLLOW)
            .union(OFlags::NONBLOCK)
            .union(OFlags::CLOEXEC),
        Mode::empty(),
    )
    .map_err(|_| GitStageError::RefFormatUnsupported)?;
    let stat = fstat(&fd).map_err(|_| GitStageError::RefFormatUnsupported)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_nlink != 1
        || stat.st_uid != uid
    {
        return Err(GitStageError::RefFormatUnsupported);
    }
    let mut file = File::from(fd);
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| GitStageError::RefFormatUnsupported)?;
    if bytes.len() > max_bytes {
        return Err(GitStageError::RefFormatUnsupported);
    }
    String::from_utf8(bytes).map_err(|_| GitStageError::RefFormatUnsupported)
}

fn validate_existing_directory(directory: &OwnedFd, uid: u32) -> Result<(), GitStageError> {
    let stat = fstat(directory).map_err(|_| GitStageError::RefFormatUnsupported)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory || stat.st_uid != uid {
        return Err(GitStageError::RefFormatUnsupported);
    }
    Ok(())
}

fn is_lower_hex_exact(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn ensure_no_alternates(repo: BorrowedFd<'_>) -> Result<(), GitStageError> {
    for name in ["objects/info/alternates", "objects/info/http-alternates"] {
        match statat(repo, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => return Err(GitStageError::AlternatesPresent),
            Err(Errno::NOENT | Errno::NOTDIR) => {}
            Err(_) => return Err(GitStageError::Preparation),
        }
    }
    Ok(())
}

fn create_staging_dir(parent: &OwnedFd) -> Result<(OwnedFd, String), GitStageError> {
    let mut selected = None;
    for attempt in 0..64_u32 {
        let name = generated_export_name(attempt);
        match mkdirat(parent, &name, Mode::from_bits_truncate(PRIVATE_DIR_MODE)) {
            Ok(()) => {
                selected = Some(name);
                break;
            }
            Err(Errno::EXIST) => continue,
            Err(_) => return Err(GitStageError::Staging),
        }
    }
    let name = selected.ok_or(GitStageError::Staging)?;
    let result = (|| {
        let dir = openat(parent, &name, DIRECTORY_OPEN_FLAGS, Mode::empty())
            .map_err(|_| GitStageError::Staging)?;
        fchmod(&dir, Mode::from_bits_truncate(PRIVATE_DIR_MODE))
            .map_err(|_| GitStageError::Staging)?;
        let stat = fstat(&dir).map_err(|_| GitStageError::Staging)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
            return Err(GitStageError::Staging);
        }
        Ok((
            parent.try_clone().map_err(|_| GitStageError::Staging)?,
            name.clone(),
        ))
    })();
    if result.is_err() {
        remove_tree(parent, &name);
    }
    result
}

fn write_bundle(
    parent: &OwnedFd,
    name: &str,
    bytes: &[u8],
    uid: u32,
) -> Result<File, GitStageError> {
    let dir = openat(parent, name, DIRECTORY_OPEN_FLAGS, Mode::empty())
        .map_err(|_| GitStageError::ExportWrite)?;
    let fd = openat(
        &dir,
        BUNDLE_FILE_NAME,
        BUNDLE_OPEN_FLAGS,
        Mode::from_bits_truncate(PRIVATE_FILE_MODE),
    )
    .map_err(|_| GitStageError::ExportWrite)?;
    fchmod(&fd, Mode::from_bits_truncate(PRIVATE_FILE_MODE))
        .map_err(|_| GitStageError::ExportWrite)?;
    let mut file = File::from(fd);
    file.write_all(bytes)
        .map_err(|_| GitStageError::ExportWrite)?;
    file.sync_all().map_err(|_| GitStageError::ExportWrite)?;
    let stat = fstat(&file).map_err(|_| GitStageError::ExportWrite)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_size < 0
        || stat.st_size as u64 != bytes.len() as u64
        || stat.st_uid != uid
        || stat.st_nlink != 1
        || stat.st_mode & 0o7777 != PRIVATE_FILE_MODE
    {
        return Err(GitStageError::ExportWrite);
    }
    Ok(file)
}

fn generated_export_name(attempt: u32) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        "git-export-{}-{nanos:x}-{counter:x}-{attempt}",
        std::process::id()
    )
}

struct StagingGuard {
    parent: Option<OwnedFd>,
    name: String,
    armed: bool,
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if self.armed
            && let Some(parent) = &self.parent
        {
            remove_tree(parent, &self.name);
        }
    }
}
