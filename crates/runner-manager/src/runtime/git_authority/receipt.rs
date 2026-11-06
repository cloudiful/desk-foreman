//! Typed, secret-free publication receipt and its private durable storage.
//!
//! The manager writes this value to `RUNNER_GIT_PRIVATE_ROOT/receipts/` after a
//! successful install or fast-forward publication and reads it back for
//! read-only reconciliation. It carries no host path, credential, URL, or raw
//! Git error; the workspace root is bound as a SHA-256 digest.

use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use runner_protocol::{
    GitReceiptPhase, GitReceiptReadAck, GitReceiptReadRequest, GitReceiptState, RunnerOwner,
    is_canonical_branch,
};
use rustix::fs::{
    AtFlags, FileType, Mode, OFlags, fchmod, fstat, fsync, mkdirat, open, openat, renameat,
};
use rustix::io::Errno;
use uuid::Uuid;

use super::runtime_config::validate_private_root;

/// Only version accepted by the typed receipt.
pub(crate) const GIT_RECEIPT_VERSION: u8 = 1;
pub(crate) const RECEIPT_DIRECTORY: &str = "receipts";
const MAX_RECEIPT_BYTES: u64 = 64 * 1024;
const RECEIPT_DIR_MODE: u32 = 0o700;
const RECEIPT_FILE_MODE: u32 = 0o600;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(1);

const DIRECTORY_OPEN_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const FILE_OPEN_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    <Option<T> as serde::Deserialize>::deserialize(deserializer)
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GitReceipt {
    pub version: u8,
    pub job_id: Uuid,
    pub operation_id: Uuid,
    pub owner: RunnerOwner,
    pub canonical_branch: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub base: Option<String>,
    pub head: String,
    pub workspace_root_sha256: String,
    pub phase: GitReceiptPhase,
    pub published: bool,
    pub cleanup_confirmed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GitReceiptError {
    InvalidVersion,
    InvalidIdentity,
    InvalidOwner,
    InvalidBranch,
    InvalidBase,
    InvalidHead,
    InvalidRootDigest,
    Unpublished,
    Conflict,
    Unavailable,
    Corrupt,
}

impl std::fmt::Display for GitReceiptError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidVersion => "git receipt version is invalid",
            Self::InvalidIdentity => "git receipt identity is invalid",
            Self::InvalidOwner => "git receipt owner is invalid",
            Self::InvalidBranch => "git receipt branch is invalid",
            Self::InvalidBase => "git receipt base is invalid",
            Self::InvalidHead => "git receipt head is invalid",
            Self::InvalidRootDigest => "git receipt root digest is invalid",
            Self::Unpublished => "git receipt is not published",
            Self::Conflict => "git receipt conflicts with existing proof",
            Self::Unavailable => "git receipt is unavailable",
            Self::Corrupt => "git receipt is corrupt",
        })
    }
}

impl std::error::Error for GitReceiptError {}

impl GitReceipt {
    pub(crate) fn validate(&self) -> Result<(), GitReceiptError> {
        if self.version != GIT_RECEIPT_VERSION {
            return Err(GitReceiptError::InvalidVersion);
        }
        if self.job_id.is_nil() || self.operation_id.is_nil() {
            return Err(GitReceiptError::InvalidIdentity);
        }
        let RunnerOwner::ApplicationPrincipal {
            application_id,
            workspace_binding_id,
            principal_digest,
        } = &self.owner
        else {
            return Err(GitReceiptError::InvalidOwner);
        };
        if *application_id <= 0
            || *workspace_binding_id <= 0
            || principal_digest.is_empty()
            || principal_digest.len() > 256
            || principal_digest
                .bytes()
                .any(|byte| byte.is_ascii_control() || byte == b' ')
        {
            return Err(GitReceiptError::InvalidOwner);
        }
        if !is_canonical_branch(&self.canonical_branch) {
            return Err(GitReceiptError::InvalidBranch);
        }
        if let Some(base) = &self.base
            && !is_oid(base)
        {
            return Err(GitReceiptError::InvalidBase);
        }
        if !is_oid(&self.head) {
            return Err(GitReceiptError::InvalidHead);
        }
        if !is_sha256_hex(&self.workspace_root_sha256) {
            return Err(GitReceiptError::InvalidRootDigest);
        }
        if self.phase == GitReceiptPhase::Completed && (!self.published || !self.cleanup_confirmed)
        {
            return Err(GitReceiptError::Unpublished);
        }
        Ok(())
    }

    /// True only for a completed, published, clean receipt.
    pub(crate) fn is_confirmed(&self) -> bool {
        self.validate().is_ok()
            && self.phase == GitReceiptPhase::Completed
            && self.published
            && self.cleanup_confirmed
    }

    /// Project this stored receipt onto the read transport acknowledgment.
    pub(crate) fn read_ack(&self) -> Result<GitReceiptReadAck, GitReceiptError> {
        self.validate()?;
        let RunnerOwner::ApplicationPrincipal {
            workspace_binding_id,
            ..
        } = &self.owner
        else {
            return Err(GitReceiptError::InvalidOwner);
        };
        Ok(GitReceiptReadAck {
            job_id: self.job_id,
            operation_id: self.operation_id,
            workspace_binding_id: *workspace_binding_id,
            state: if self.is_confirmed() {
                GitReceiptState::Confirmed
            } else {
                GitReceiptState::Unconfirmed
            },
            phase: Some(self.phase),
            base: self.base.clone(),
            head: Some(self.head.clone()),
            published: self.published,
            cleanup_confirmed: self.cleanup_confirmed,
        })
    }

    /// Rebuild a request identity for one stored receipt.
    pub(crate) fn read_request(
        &self,
        assigned_manager_id: i64,
        workspace_root: &str,
        deadline_unix_ms: i64,
    ) -> GitReceiptReadRequest {
        GitReceiptReadRequest {
            job_id: self.job_id,
            operation_id: self.operation_id,
            assigned_manager_id,
            owner: self.owner.clone(),
            canonical_branch: self.canonical_branch.clone(),
            expected_base: self.base.clone(),
            workspace_root: workspace_root.to_owned(),
            deadline_unix_ms,
        }
    }

    /// True when this receipt proves the same operation identity and root.
    pub(crate) fn matches_request(
        &self,
        request: &GitReceiptReadRequest,
    ) -> Result<(), GitReceiptError> {
        self.validate()?;
        let Some(binding) = request.workspace_binding_id() else {
            return Err(GitReceiptError::InvalidOwner);
        };
        let RunnerOwner::ApplicationPrincipal {
            workspace_binding_id,
            ..
        } = &self.owner
        else {
            return Err(GitReceiptError::InvalidOwner);
        };
        if self.job_id != request.job_id
            || self.operation_id != request.operation_id
            || *workspace_binding_id != binding
            || self.owner != request.owner
            || self.canonical_branch != request.canonical_branch
            || self.base != request.expected_base
            || self.workspace_root_sha256 != workspace_root_sha256(&request.workspace_root)
        {
            return Err(GitReceiptError::Conflict);
        }
        Ok(())
    }
}

fn is_oid(value: &str) -> bool {
    let len = value.len();
    (len == 40 || len == 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) use runner_protocol::git_authority::workspace_root_sha256;

pub(crate) fn receipt_path(private_root: &Path, job_id: Uuid) -> PathBuf {
    private_root
        .join(RECEIPT_DIRECTORY)
        .join(format!("{job_id}.json"))
}

/// Read one receipt without writing, repairing, or replaying anything.
pub(crate) fn read_receipt(
    private_root: &Path,
    request: &GitReceiptReadRequest,
) -> Result<GitReceipt, GitReceiptError> {
    let euid = current_euid();
    let root =
        validate_private_root(private_root, euid).map_err(|_| GitReceiptError::Unavailable)?;
    let root_fd = open(&root, DIRECTORY_OPEN_FLAGS, Mode::empty())
        .map_err(|_| GitReceiptError::Unavailable)?;
    let directory = openat(
        &root_fd,
        RECEIPT_DIRECTORY,
        DIRECTORY_OPEN_FLAGS,
        Mode::empty(),
    )
    .map_err(|_| GitReceiptError::Unavailable)?;
    let name = receipt_name(request.job_id);
    let file =
        openat(&directory, &name, FILE_OPEN_FLAGS, Mode::empty()).map_err(|error| match error {
            Errno::NOENT => GitReceiptError::Unavailable,
            _ => GitReceiptError::Corrupt,
        })?;
    let stat = fstat(&file).map_err(|_| GitReceiptError::Corrupt)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != euid
        || stat.st_nlink != 1
        || u64::try_from(stat.st_size).unwrap_or(u64::MAX) > MAX_RECEIPT_BYTES
        || (stat.st_mode as u32) & 0o077 != 0
    {
        return Err(GitReceiptError::Corrupt);
    }
    let mut bytes = Vec::with_capacity(stat.st_size as usize);
    let mut file = std::fs::File::from(file);
    file.read_to_end(&mut bytes)
        .map_err(|_| GitReceiptError::Corrupt)?;
    let receipt: GitReceipt =
        serde_json::from_slice(&bytes).map_err(|_| GitReceiptError::Corrupt)?;
    receipt.validate()?;
    receipt.matches_request(request)?;
    Ok(receipt)
}

/// Persist one phase receipt atomically, preserving a stronger completed proof.
pub(crate) fn write_receipt(
    private_root: &Path,
    receipt: &GitReceipt,
) -> Result<(), GitReceiptError> {
    receipt.validate()?;
    let euid = current_euid();
    let root =
        validate_private_root(private_root, euid).map_err(|_| GitReceiptError::Unavailable)?;
    let root_fd = open(&root, DIRECTORY_OPEN_FLAGS, Mode::empty())
        .map_err(|_| GitReceiptError::Unavailable)?;
    let directory = match openat(
        &root_fd,
        RECEIPT_DIRECTORY,
        DIRECTORY_OPEN_FLAGS,
        Mode::empty(),
    ) {
        Ok(directory) => directory,
        Err(Errno::NOENT) => {
            mkdirat(
                &root_fd,
                RECEIPT_DIRECTORY,
                Mode::from_bits_truncate(RECEIPT_DIR_MODE),
            )
            .map_err(|_| GitReceiptError::Unavailable)?;
            let directory = openat(
                &root_fd,
                RECEIPT_DIRECTORY,
                DIRECTORY_OPEN_FLAGS,
                Mode::empty(),
            )
            .map_err(|_| GitReceiptError::Unavailable)?;
            fchmod(&directory, Mode::from_bits_truncate(RECEIPT_DIR_MODE))
                .map_err(|_| GitReceiptError::Unavailable)?;
            directory
        }
        Err(_) => return Err(GitReceiptError::Unavailable),
    };
    let directory_stat = fstat(&directory).map_err(|_| GitReceiptError::Unavailable)?;
    if FileType::from_raw_mode(directory_stat.st_mode) != FileType::Directory
        || directory_stat.st_uid != euid
        || (directory_stat.st_mode as u32) & 0o777 != RECEIPT_DIR_MODE
    {
        return Err(GitReceiptError::Unavailable);
    }
    let name = receipt_name(receipt.job_id);
    match read_existing(&directory, &name, euid) {
        Ok(existing) => ensure_transition(&existing, receipt)?,
        Err(GitReceiptError::Unavailable) => {}
        Err(error) => return Err(error),
    }
    let bytes = serde_json::to_vec(receipt).map_err(|_| GitReceiptError::Corrupt)?;
    let temp_name = format!(
        ".{}.tmp.{}.{}",
        receipt.job_id,
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let flags = OFlags::WRONLY
        .union(OFlags::CREATE)
        .union(OFlags::EXCL)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);
    let temp = match openat(
        &directory,
        &temp_name,
        flags,
        Mode::from_bits_truncate(RECEIPT_FILE_MODE),
    ) {
        Ok(fd) => fd,
        Err(_) => return Err(GitReceiptError::Unavailable),
    };
    let result = (|| {
        fchmod(&temp, Mode::from_bits_truncate(RECEIPT_FILE_MODE))
            .map_err(|_| GitReceiptError::Unavailable)?;
        let mut file =
            std::fs::File::from(temp.try_clone().map_err(|_| GitReceiptError::Unavailable)?);
        file.write_all(&bytes)
            .map_err(|_| GitReceiptError::Unavailable)?;
        file.sync_all().map_err(|_| GitReceiptError::Unavailable)?;
        fsync(&directory).map_err(|_| GitReceiptError::Unavailable)?;
        renameat(&directory, &temp_name, &directory, &name)
            .map_err(|_| GitReceiptError::Unavailable)
    })();
    if result.is_err() {
        let _ = rustix::fs::unlinkat(&directory, &temp_name, AtFlags::empty());
    }
    result
}

fn read_existing(
    directory: &OwnedFd,
    name: &str,
    euid: u32,
) -> Result<GitReceipt, GitReceiptError> {
    let file = openat(directory, name, FILE_OPEN_FLAGS, Mode::empty())
        .map_err(|_| GitReceiptError::Unavailable)?;
    let stat = fstat(&file).map_err(|_| GitReceiptError::Corrupt)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != euid
        || stat.st_nlink != 1
        || u64::try_from(stat.st_size).unwrap_or(u64::MAX) > MAX_RECEIPT_BYTES
        || (stat.st_mode as u32) & 0o077 != 0
    {
        return Err(GitReceiptError::Corrupt);
    }
    let mut bytes = Vec::with_capacity(stat.st_size as usize);
    let mut file = std::fs::File::from(file);
    file.read_to_end(&mut bytes)
        .map_err(|_| GitReceiptError::Corrupt)?;
    let receipt: GitReceipt =
        serde_json::from_slice(&bytes).map_err(|_| GitReceiptError::Corrupt)?;
    receipt.validate()?;
    Ok(receipt)
}

fn ensure_transition(existing: &GitReceipt, next: &GitReceipt) -> Result<(), GitReceiptError> {
    if existing.job_id != next.job_id
        || existing.operation_id != next.operation_id
        || existing.owner != next.owner
        || existing.canonical_branch != next.canonical_branch
        || existing.base != next.base
        || existing.head != next.head
        || existing.workspace_root_sha256 != next.workspace_root_sha256
    {
        return Err(GitReceiptError::Conflict);
    }
    match (existing.phase, next.phase) {
        (GitReceiptPhase::Completed, GitReceiptPhase::Verified) => Err(GitReceiptError::Conflict),
        (GitReceiptPhase::Completed, GitReceiptPhase::Completed) => {
            if existing.published == next.published
                && existing.cleanup_confirmed == next.cleanup_confirmed
            {
                Ok(())
            } else {
                Err(GitReceiptError::Conflict)
            }
        }
        _ => Ok(()),
    }
}

fn receipt_name(job_id: Uuid) -> String {
    format!("{job_id}.json")
}

pub(crate) fn current_euid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self")
        .map(|metadata| metadata.uid())
        .unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
    const BASE: &str = "fedcba9876543210fedcba9876543210fedcba98";

    fn receipt() -> GitReceipt {
        GitReceipt {
            version: GIT_RECEIPT_VERSION,
            job_id: Uuid::new_v4(),
            operation_id: Uuid::new_v4(),
            owner: RunnerOwner::ApplicationPrincipal {
                application_id: 7,
                workspace_binding_id: 11,
                principal_digest: "a".repeat(64),
            },
            canonical_branch: "main".to_owned(),
            base: Some(BASE.to_owned()),
            head: HEAD.to_owned(),
            workspace_root_sha256: workspace_root_sha256("/srv/workspaces/binding-11"),
            phase: GitReceiptPhase::Completed,
            published: true,
            cleanup_confirmed: true,
        }
    }

    fn private_root() -> tempfile::TempDir {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().expect("private root");
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
            .expect("root mode");
        root
    }

    #[test]
    fn receipt_roundtrips_and_rejects_unknown_fields() {
        let receipt = receipt();
        let json = serde_json::to_string(&receipt).expect("serialize");
        assert!(!json.contains("password"));
        assert!(!json.contains("workspace_root\":"));
        assert_eq!(
            serde_json::from_str::<GitReceipt>(&json).expect("deserialize"),
            receipt
        );
        receipt.validate().expect("valid receipt");
        assert!(receipt.is_confirmed());

        let unknown = json.replace('{', "{\"receipt_path\":\"/tmp/receipt\",");
        assert!(serde_json::from_str::<GitReceipt>(&unknown).is_err());
        let missing_base =
            json.replace("\"base\":\"fedcba9876543210fedcba9876543210fedcba98\",", "");
        assert!(serde_json::from_str::<GitReceipt>(&missing_base).is_err());
    }

    #[test]
    fn receipt_io_is_private_atomic_and_root_bound() {
        let root = private_root();
        let mut receipt = receipt();
        receipt.phase = GitReceiptPhase::Verified;
        receipt.published = false;
        receipt.cleanup_confirmed = false;
        write_receipt(root.path(), &receipt).expect("write verified");
        let path = receipt_path(root.path(), receipt.job_id);
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(&path).expect("receipt metadata");
        assert_eq!(metadata.mode() & 0o777, 0o600);
        assert_eq!(metadata.nlink(), 1);
        let request = receipt.read_request(9, "/srv/workspaces/binding-11", 1_800_000_000_000);
        let stored = read_receipt(root.path(), &request).expect("read receipt");
        assert_eq!(stored, receipt);

        let mut completed = receipt.clone();
        completed.phase = GitReceiptPhase::Completed;
        completed.published = true;
        completed.cleanup_confirmed = true;
        write_receipt(root.path(), &completed).expect("upgrade to completed");
        assert!(
            read_receipt(root.path(), &request)
                .expect("read")
                .is_confirmed()
        );

        let mut downgrade = completed.clone();
        downgrade.phase = GitReceiptPhase::Verified;
        downgrade.published = false;
        downgrade.cleanup_confirmed = false;
        assert_eq!(
            write_receipt(root.path(), &downgrade),
            Err(GitReceiptError::Conflict)
        );

        let mut other_root =
            completed.read_request(9, "/srv/workspaces/binding-12", 1_800_000_000_000);
        other_root.job_id = completed.job_id;
        assert_eq!(
            read_receipt(root.path(), &other_root),
            Err(GitReceiptError::Conflict)
        );
    }

    #[test]
    fn receipt_rejects_wrong_identity_and_insecure_file() {
        use std::os::unix::fs::PermissionsExt;
        let root = private_root();
        let receipt = receipt();
        write_receipt(root.path(), &receipt).expect("write");
        let mut request = receipt.read_request(9, "/srv/workspaces/binding-11", 1_800_000_000_000);
        request.operation_id = Uuid::new_v4();
        assert_eq!(
            read_receipt(root.path(), &request),
            Err(GitReceiptError::Conflict)
        );

        let path = receipt_path(root.path(), receipt.job_id);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("mode");
        let request = receipt.read_request(9, "/srv/workspaces/binding-11", 1_800_000_000_000);
        assert_eq!(
            read_receipt(root.path(), &request),
            Err(GitReceiptError::Corrupt)
        );
    }

    #[test]
    fn receipt_requires_v1_identity_and_valid_oids() {
        let mut bad = receipt();
        bad.version = 2;
        assert_eq!(bad.validate(), Err(GitReceiptError::InvalidVersion));

        let mut bad = receipt();
        bad.operation_id = Uuid::nil();
        assert_eq!(bad.validate(), Err(GitReceiptError::InvalidIdentity));

        let mut bad = receipt();
        bad.owner = RunnerOwner::WorkspaceBinding {
            workspace_binding_id: 11,
        };
        assert_eq!(bad.validate(), Err(GitReceiptError::InvalidOwner));

        let mut bad = receipt();
        bad.head = "A".repeat(40);
        assert_eq!(bad.validate(), Err(GitReceiptError::InvalidHead));

        let mut bad = receipt();
        bad.base = Some("b".repeat(41));
        assert_eq!(bad.validate(), Err(GitReceiptError::InvalidBase));

        let mut bad = receipt();
        bad.canonical_branch = "-main".to_owned();
        assert_eq!(bad.validate(), Err(GitReceiptError::InvalidBranch));

        let mut bad = receipt();
        bad.workspace_root_sha256 = "A".repeat(64);
        assert_eq!(bad.validate(), Err(GitReceiptError::InvalidRootDigest));
    }

    #[test]
    fn only_completed_published_clean_receipts_confirm() {
        let mut verified = receipt();
        verified.phase = GitReceiptPhase::Verified;
        verified.published = false;
        verified.cleanup_confirmed = false;
        verified.validate().expect("verified phase");
        assert!(!verified.is_confirmed());

        let mut unpublished = receipt();
        unpublished.published = false;
        assert_eq!(unpublished.validate(), Err(GitReceiptError::Unpublished));

        let mut dirty = receipt();
        dirty.cleanup_confirmed = false;
        assert_eq!(dirty.validate(), Err(GitReceiptError::Unpublished));
    }

    #[test]
    fn receipt_projects_to_a_confirmed_ack_for_its_own_request() {
        let receipt = receipt();
        let request = receipt.read_request(9, "/srv/workspaces/binding-11", 1_800_000_000_000);
        request.validate().expect("request");
        let ack = receipt.read_ack().expect("ack");
        assert_eq!(ack.state, GitReceiptState::Confirmed);
        assert!(ack.confirms(&request));
    }
}
