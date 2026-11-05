use uuid::Uuid;

use crate::RunnerOwner;

/// Object format recorded by the verification request and confirmed by the
/// trusted Git runner.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitObjectFormat {
    Sha1,
    Sha256,
}

impl GitObjectFormat {
    pub fn hash_hex_len(self) -> usize {
        match self {
            Self::Sha1 => 40,
            Self::Sha256 => 64,
        }
    }

    pub fn git_name(self) -> &'static str {
        match self {
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
        }
    }
}

/// Secret-free request to verify one raw Git snapshot.
///
/// This is structural input only. Proof of server-side ownership and a live
/// lease belongs to the caller; deserializing this request never grants either.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitVerifyRequest {
    pub owner: RunnerOwner,
    pub workspace_binding_id: i64,
    pub code_project_id: Uuid,
    pub authorization_generation: Uuid,
    pub expected_base: Option<String>,
    pub expected_new_head: String,
    pub branch: String,
    pub object_format: GitObjectFormat,
    pub snapshot_name: String,
}

/// Result of a completed verification sequence.
///
/// Deliberately not deserializable: only the trusted verifier may construct
/// this as evidence after the semantic checks have run.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct GitVerifyResult {
    pub head: String,
    pub branch: String,
    pub base: Option<String>,
    pub object_format: GitObjectFormat,
}

/// Structural availability of the trusted Git runtime.
///
/// These states are informational only. No state grants Git execution,
/// Docker readiness, or verified isolation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitRuntimeStatus {
    Unconfigured,
    Invalid,
    ConfiguredUnverified,
}

/// Fixed availability failure classes. They carry no paths, images, hosts,
/// credentials, or configuration values.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitRuntimeReason {
    ManagerUidUnavailable,
    ManagerRunningAsRoot,
    BackendUnsupported,
    ImageMissing,
    ImageNotDigestPinned,
    ImageInvalid,
    LimitsInvalid,
    PrivateRootMissing,
    PrivateRootNotAbsolute,
    PrivateRootNotNormalized,
    PrivateRootUnavailable,
    PrivateRootSymlink,
    PrivateRootNotDirectory,
    PrivateRootNotOwned,
    PrivateRootNotPrivate,
    PrivateRootAncestorUnsafe,
    PrivateRootOverlapsWorkspace,
    WorkspaceRootUnproven,
    DockerHostUnsupported,
    HostRootRequiredForRemoteDaemon,
    HostRootNotAbsolute,
    HostRootNotNormalized,
}

/// Two-field availability response for the authenticated manager route.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitRuntimeAvailability {
    pub status: GitRuntimeStatus,
    pub reason: Option<GitRuntimeReason>,
}

/// Fixed validation failure classes. They carry no paths, refs, or payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitVerifyRequestError {
    InvalidOwner,
    InvalidWorkspaceBinding,
    InvalidCodeProject,
    InvalidGeneration,
    InvalidHead,
    InvalidBase,
    InvalidBranch,
    InvalidSnapshotName,
}

impl std::fmt::Display for GitVerifyRequestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::InvalidOwner => "git verify owner is invalid",
            Self::InvalidWorkspaceBinding => "git verify workspace binding is invalid",
            Self::InvalidCodeProject => "git verify code project is invalid",
            Self::InvalidGeneration => "git verify authorization generation is invalid",
            Self::InvalidHead => "git verify expected new head is invalid",
            Self::InvalidBase => "git verify expected base is invalid",
            Self::InvalidBranch => "git verify branch is invalid",
            Self::InvalidSnapshotName => "git verify snapshot name is invalid",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for GitVerifyRequestError {}

impl GitVerifyRequest {
    /// Validate the request's structural shape. This does not prove ownership,
    /// lease liveness, or snapshot provenance.
    pub fn validate(&self) -> Result<(), GitVerifyRequestError> {
        if self.workspace_binding_id <= 0 {
            return Err(GitVerifyRequestError::InvalidWorkspaceBinding);
        }
        self.validate_owner()?;
        if self.code_project_id.is_nil() {
            return Err(GitVerifyRequestError::InvalidCodeProject);
        }
        if self.authorization_generation.is_nil() {
            return Err(GitVerifyRequestError::InvalidGeneration);
        }
        let hash_len = self.object_format.hash_hex_len();
        if !is_lower_hex_exact(&self.expected_new_head, hash_len) {
            return Err(GitVerifyRequestError::InvalidHead);
        }
        if let Some(base) = &self.expected_base
            && !is_lower_hex_exact(base, hash_len)
        {
            return Err(GitVerifyRequestError::InvalidBase);
        }
        if !is_canonical_branch(&self.branch) {
            return Err(GitVerifyRequestError::InvalidBranch);
        }
        if !is_generated_snapshot_name(&self.snapshot_name) {
            return Err(GitVerifyRequestError::InvalidSnapshotName);
        }
        Ok(())
    }

    fn validate_owner(&self) -> Result<(), GitVerifyRequestError> {
        let RunnerOwner::ApplicationPrincipal {
            application_id,
            workspace_binding_id,
            principal_digest,
        } = &self.owner
        else {
            return Err(GitVerifyRequestError::InvalidOwner);
        };
        if *application_id <= 0
            || *workspace_binding_id <= 0
            || *workspace_binding_id != self.workspace_binding_id
        {
            return Err(GitVerifyRequestError::InvalidOwner);
        }
        if principal_digest.is_empty()
            || principal_digest.len() > 256
            || principal_digest
                .bytes()
                .any(|byte| byte.is_ascii_control() || byte == b' ')
        {
            return Err(GitVerifyRequestError::InvalidOwner);
        }
        Ok(())
    }
}

/// Deserialize an `Option` that must be present on the wire, including `null`.
///
/// Without this, serde silently treats a missing field as `None`, which would
/// make old or malformed success payloads look like a first clone.
pub fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    <Option<T> as serde::Deserialize>::deserialize(deserializer)
}

fn is_lower_hex_exact(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn is_canonical_branch(value: &str) -> bool {
    if value.is_empty() || value.len() > 255 {
        return false;
    }
    if value.starts_with('-')
        || value.starts_with('/')
        || value.ends_with('/')
        || value.starts_with('.')
        || value.ends_with('.')
        || value.ends_with(".lock")
        || value.contains("..")
        || value.contains("@{")
        || value.contains("//")
    {
        return false;
    }
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
}

fn is_generated_snapshot_name(value: &str) -> bool {
    if value.is_empty() || value.len() > 128 || value.contains('/') || value.contains("..") {
        return false;
    }
    let mut parts = value.split('-');
    let (Some("raw"), Some("git"), Some(pid), Some(nanos), Some(counter), Some(attempt), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return false;
    };
    !pid.is_empty()
        && pid.bytes().all(|byte| byte.is_ascii_digit())
        && !nanos.is_empty()
        && nanos
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && !counter.is_empty()
        && counter
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && !attempt.is_empty()
        && attempt.bytes().all(|byte| byte.is_ascii_digit())
}

/// Closed phase recorded by a durable publication receipt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitReceiptPhase {
    Verified,
    Completed,
}

/// Closed result state of one read-only receipt lookup.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitReceiptState {
    Confirmed,
    Unconfirmed,
    Missing,
}

/// Secret-free request to read one manager-side receipt.
///
/// The workspace binding is derived from `owner`; carrying both would allow a
/// mismatched identity. Structural validation never proves receipt existence.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitReceiptReadRequest {
    pub job_id: Uuid,
    pub operation_id: Uuid,
    pub assigned_manager_id: i64,
    pub owner: RunnerOwner,
    pub canonical_branch: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub expected_base: Option<String>,
    pub workspace_root: String,
    pub deadline_unix_ms: i64,
}

/// Closed validation classes for a receipt read request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitReceiptReadRequestError {
    InvalidJob,
    InvalidOperation,
    InvalidManager,
    InvalidOwner,
    InvalidBranch,
    InvalidBase,
    InvalidWorkspaceRoot,
    InvalidDeadline,
}

impl std::fmt::Display for GitReceiptReadRequestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidJob => "git receipt read job id is invalid",
            Self::InvalidOperation => "git receipt read operation id is invalid",
            Self::InvalidManager => "git receipt read manager is invalid",
            Self::InvalidOwner => "git receipt read owner is invalid",
            Self::InvalidBranch => "git receipt read branch is invalid",
            Self::InvalidBase => "git receipt read expected base is invalid",
            Self::InvalidWorkspaceRoot => "git receipt read workspace root is invalid",
            Self::InvalidDeadline => "git receipt read deadline is invalid",
        })
    }
}

impl std::error::Error for GitReceiptReadRequestError {}

impl GitReceiptReadRequest {
    /// The single binding identity carried by the owner.
    pub fn workspace_binding_id(&self) -> Option<i64> {
        match &self.owner {
            RunnerOwner::ApplicationPrincipal {
                workspace_binding_id,
                ..
            } => Some(*workspace_binding_id),
            _ => None,
        }
    }

    pub fn validate(&self) -> Result<(), GitReceiptReadRequestError> {
        if self.job_id.is_nil() {
            return Err(GitReceiptReadRequestError::InvalidJob);
        }
        if self.operation_id.is_nil() {
            return Err(GitReceiptReadRequestError::InvalidOperation);
        }
        if self.assigned_manager_id <= 0 {
            return Err(GitReceiptReadRequestError::InvalidManager);
        }
        let RunnerOwner::ApplicationPrincipal {
            application_id,
            workspace_binding_id,
            principal_digest,
        } = &self.owner
        else {
            return Err(GitReceiptReadRequestError::InvalidOwner);
        };
        if *application_id <= 0
            || *workspace_binding_id <= 0
            || principal_digest.is_empty()
            || principal_digest.len() > 256
            || principal_digest
                .bytes()
                .any(|byte| byte.is_ascii_control() || byte == b' ')
        {
            return Err(GitReceiptReadRequestError::InvalidOwner);
        }
        if !is_canonical_branch(&self.canonical_branch) {
            return Err(GitReceiptReadRequestError::InvalidBranch);
        }
        if let Some(base) = &self.expected_base
            && !(is_lower_hex_exact(base, 40) || is_lower_hex_exact(base, 64))
        {
            return Err(GitReceiptReadRequestError::InvalidBase);
        }
        if self.workspace_root.is_empty()
            || self.workspace_root.len() > GIT_JOB_WORKSPACE_ROOT_MAX_LEN
            || self.workspace_root.contains('\0')
            || !std::path::Path::new(&self.workspace_root).is_absolute()
        {
            return Err(GitReceiptReadRequestError::InvalidWorkspaceRoot);
        }
        if self.deadline_unix_ms <= 0 {
            return Err(GitReceiptReadRequestError::InvalidDeadline);
        }
        Ok(())
    }
}

/// Secret-free acknowledgment for one read-only receipt lookup.
///
/// Only [`GitReceiptState::Confirmed`] with a completed phase, a canonical
/// head, publication and confirmed cleanup may promote authority.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitReceiptReadAck {
    pub job_id: Uuid,
    pub operation_id: Uuid,
    pub workspace_binding_id: i64,
    pub state: GitReceiptState,
    pub phase: Option<GitReceiptPhase>,
    #[serde(deserialize_with = "deserialize_required_option")]
    pub base: Option<String>,
    pub head: Option<String>,
    pub published: bool,
    pub cleanup_confirmed: bool,
}

/// Closed validation classes for a receipt read acknowledgment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitReceiptReadAckError {
    InvalidIdentity,
    InvalidHead,
    InvalidBase,
    Unconfirmed,
}

impl std::fmt::Display for GitReceiptReadAckError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidIdentity => "git receipt ack identity is invalid",
            Self::InvalidHead => "git receipt ack head is invalid",
            Self::InvalidBase => "git receipt ack base is invalid",
            Self::Unconfirmed => "git receipt is not confirmed",
        })
    }
}

impl std::error::Error for GitReceiptReadAckError {}

impl GitReceiptReadAck {
    pub fn validate(&self) -> Result<(), GitReceiptReadAckError> {
        if self.job_id.is_nil() || self.operation_id.is_nil() || self.workspace_binding_id <= 0 {
            return Err(GitReceiptReadAckError::InvalidIdentity);
        }
        if let Some(head) = &self.head
            && !(is_lower_hex_exact(head, 40) || is_lower_hex_exact(head, 64))
        {
            return Err(GitReceiptReadAckError::InvalidHead);
        }
        if let Some(base) = &self.base
            && !(is_lower_hex_exact(base, 40) || is_lower_hex_exact(base, 64))
        {
            return Err(GitReceiptReadAckError::InvalidBase);
        }
        if self.state == GitReceiptState::Confirmed
            && (self.phase != Some(GitReceiptPhase::Completed)
                || self.head.is_none()
                || !self.published
                || !self.cleanup_confirmed)
        {
            return Err(GitReceiptReadAckError::Unconfirmed);
        }
        Ok(())
    }

    /// Validate the ack shape and prove every identity matches the request.
    pub fn validate_matches(
        &self,
        request: &GitReceiptReadRequest,
    ) -> Result<(), GitReceiptReadAckError> {
        self.validate()?;
        if self.job_id != request.job_id
            || self.operation_id != request.operation_id
            || Some(self.workspace_binding_id) != request.workspace_binding_id()
        {
            return Err(GitReceiptReadAckError::InvalidIdentity);
        }
        Ok(())
    }

    /// True only for a confirmed, completed, published, clean ack for this
    /// exact request. Missing and unconfirmed acks never promote authority.
    pub fn confirms(&self, request: &GitReceiptReadRequest) -> bool {
        self.validate_matches(request).is_ok()
            && self.state == GitReceiptState::Confirmed
            && self.phase == Some(GitReceiptPhase::Completed)
            && self.published
            && self.cleanup_confirmed
            && self.base == request.expected_base
            && self
                .head
                .as_deref()
                .is_some_and(|head| is_lower_hex_exact(head, 40) || is_lower_hex_exact(head, 64))
    }
}

/// Stable SHA-256 digest of the validated canonical workspace root bytes.
///
/// The wire carries the digest instead of a host path so a caller cannot probe
/// or reuse another binding's root spelling.
pub fn workspace_root_sha256(root: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(root.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Maximum accepted length of the authoritative workspace root on the wire.
pub const GIT_JOB_WORKSPACE_ROOT_MAX_LEN: usize = 4096;

/// Maximum accepted length of the credential-free remote URL.
pub const GIT_JOB_CREDENTIAL_URL_MAX_LEN: usize = 2048;

/// Fixed Git broker username. The credential carries no account selection.
pub const GIT_JOB_CREDENTIAL_USERNAME: &str = "broker";

/// Secret-free claim payload delivered to the assigned runner manager.
///
/// Derived only from the claimed row and the authoritative binding: it has no
/// broker credential, no remote URL, and no row token. The owner is the
/// application principal that admitted the job.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitJobSpec {
    pub job_id: Uuid,
    pub operation_id: Uuid,
    pub owner: RunnerOwner,
    pub application_id: i64,
    pub workspace_binding_id: i64,
    pub principal_digest: String,
    pub code_project_id: Uuid,
    pub authorization_generation: Uuid,
    pub canonical_branch: String,
    pub expected_base: Option<String>,
    pub reserved_bytes: i64,
    pub deadline_unix_ms: i64,
    pub workspace_root: String,
}

/// Closed validation classes for a Git job claim payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitJobSpecError {
    InvalidJob,
    InvalidOperation,
    InvalidOwner,
    InvalidApplication,
    InvalidBinding,
    InvalidPrincipal,
    InvalidCodeProject,
    InvalidGeneration,
    InvalidBranch,
    InvalidBase,
    InvalidReservation,
    InvalidDeadline,
    InvalidWorkspaceRoot,
}

impl std::fmt::Display for GitJobSpecError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidJob => "git job id is invalid",
            Self::InvalidOperation => "git job operation id is invalid",
            Self::InvalidOwner => "git job owner is invalid",
            Self::InvalidApplication => "git job application is invalid",
            Self::InvalidBinding => "git job workspace binding is invalid",
            Self::InvalidPrincipal => "git job principal is invalid",
            Self::InvalidCodeProject => "git job code project is invalid",
            Self::InvalidGeneration => "git job authorization generation is invalid",
            Self::InvalidBranch => "git job branch is invalid",
            Self::InvalidBase => "git job expected base is invalid",
            Self::InvalidReservation => "git job reservation is invalid",
            Self::InvalidDeadline => "git job deadline is invalid",
            Self::InvalidWorkspaceRoot => "git job workspace root is invalid",
        })
    }
}

impl std::error::Error for GitJobSpecError {}

impl GitJobSpec {
    pub fn validate(&self) -> Result<(), GitJobSpecError> {
        if self.job_id.is_nil() {
            return Err(GitJobSpecError::InvalidJob);
        }
        if self.operation_id.is_nil() {
            return Err(GitJobSpecError::InvalidOperation);
        }
        let RunnerOwner::ApplicationPrincipal {
            application_id,
            workspace_binding_id,
            principal_digest,
        } = &self.owner
        else {
            return Err(GitJobSpecError::InvalidOwner);
        };
        if *application_id != self.application_id || self.application_id <= 0 {
            return Err(GitJobSpecError::InvalidApplication);
        }
        if *workspace_binding_id != self.workspace_binding_id || self.workspace_binding_id <= 0 {
            return Err(GitJobSpecError::InvalidBinding);
        }
        if principal_digest != &self.principal_digest
            || !is_lower_hex_exact(&self.principal_digest, 64)
        {
            return Err(GitJobSpecError::InvalidPrincipal);
        }
        if self.code_project_id.is_nil() {
            return Err(GitJobSpecError::InvalidCodeProject);
        }
        if self.authorization_generation.is_nil() {
            return Err(GitJobSpecError::InvalidGeneration);
        }
        if !is_canonical_branch(&self.canonical_branch) {
            return Err(GitJobSpecError::InvalidBranch);
        }
        if let Some(base) = &self.expected_base
            && !(is_lower_hex_exact(base, 40) || is_lower_hex_exact(base, 64))
        {
            return Err(GitJobSpecError::InvalidBase);
        }
        if self.reserved_bytes <= 0 {
            return Err(GitJobSpecError::InvalidReservation);
        }
        if self.deadline_unix_ms <= 0 {
            return Err(GitJobSpecError::InvalidDeadline);
        }
        if self.workspace_root.is_empty()
            || self.workspace_root.len() > GIT_JOB_WORKSPACE_ROOT_MAX_LEN
            || self.workspace_root.contains('\0')
            || !std::path::Path::new(&self.workspace_root).is_absolute()
        {
            return Err(GitJobSpecError::InvalidWorkspaceRoot);
        }
        Ok(())
    }
}

/// Closed failure classes a manager may report for a Git job.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitJobErrorCode {
    Unavailable,
    CredentialUnavailable,
    Timeout,
    Cancelled,
    ResourceLimit,
    WorkspaceBusy,
    Conflict,
    ExecutionFailed,
}

impl GitJobErrorCode {
    /// Fixed bounded summary persisted with a failed job.
    pub fn summary(self) -> &'static str {
        match self {
            Self::Unavailable => "git operation unavailable",
            Self::CredentialUnavailable => "git credential unavailable",
            Self::Timeout => "git operation timed out",
            Self::Cancelled => "git operation cancelled",
            Self::ResourceLimit => "git resource limit exceeded",
            Self::WorkspaceBusy => "workspace busy",
            Self::Conflict => "git operation conflict",
            Self::ExecutionFailed => "git operation failed",
        }
    }
}

/// Closed result payload reported by the assigned manager.
///
/// `operation_id` binds the result to the exact claimed operation. The outer
/// runner result must agree with the typed variant.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum GitJobOutcome {
    SyncSucceeded {
        operation_id: Uuid,
        head: String,
        #[serde(deserialize_with = "deserialize_required_option")]
        base: Option<String>,
        published: bool,
        cleanup_confirmed: bool,
    },
    Failed {
        operation_id: Uuid,
        error_code: GitJobErrorCode,
        cleanup_confirmed: bool,
    },
}

/// Closed validation classes for a Git job result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitJobOutcomeError {
    InvalidOperation,
    InvalidHead,
    InvalidBase,
    Unpublished,
}

impl std::fmt::Display for GitJobOutcomeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidOperation => "git job operation id is invalid",
            Self::InvalidHead => "git job result head is invalid",
            Self::InvalidBase => "git job result base is invalid",
            Self::Unpublished => "git job result is not published",
        })
    }
}

impl std::error::Error for GitJobOutcomeError {}

impl GitJobOutcome {
    pub fn operation_id(&self) -> Uuid {
        match self {
            Self::SyncSucceeded { operation_id, .. } | Self::Failed { operation_id, .. } => {
                *operation_id
            }
        }
    }

    pub fn cleanup_confirmed(&self) -> bool {
        match self {
            Self::SyncSucceeded {
                cleanup_confirmed, ..
            }
            | Self::Failed {
                cleanup_confirmed, ..
            } => *cleanup_confirmed,
        }
    }

    pub fn validate(&self) -> Result<(), GitJobOutcomeError> {
        if self.operation_id().is_nil() {
            return Err(GitJobOutcomeError::InvalidOperation);
        }
        if let Self::SyncSucceeded {
            head,
            base,
            published,
            ..
        } = self
        {
            if !(is_lower_hex_exact(head, 40) || is_lower_hex_exact(head, 64)) {
                return Err(GitJobOutcomeError::InvalidHead);
            }
            if let Some(base) = base
                && !(is_lower_hex_exact(base, 40) || is_lower_hex_exact(base, 64))
            {
                return Err(GitJobOutcomeError::InvalidBase);
            }
            if !published {
                return Err(GitJobOutcomeError::Unpublished);
            }
        }
        Ok(())
    }
}

/// The only secret-carrying type in the control plane.
///
/// Field limits and URL scope are validated by the server-side credential
/// store. It deliberately has no schema derive and no display implementation,
/// and its `Debug` reveals only the type name.
#[derive(Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitJobCredential {
    pub username: String,
    pub password: String,
    pub remote_url: String,
    pub deadline_unix_ms: i64,
}

impl std::fmt::Debug for GitJobCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("GitJobCredential(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
    const OTHER: &str = "fedcba9876543210fedcba9876543210fedcba98";

    fn owner(binding: i64) -> RunnerOwner {
        RunnerOwner::ApplicationPrincipal {
            application_id: 7,
            workspace_binding_id: binding,
            principal_digest: "principal-digest".to_owned(),
        }
    }

    fn sample_request() -> GitVerifyRequest {
        GitVerifyRequest {
            owner: owner(11),
            workspace_binding_id: 11,
            code_project_id: Uuid::new_v4(),
            authorization_generation: Uuid::new_v4(),
            expected_base: None,
            expected_new_head: HEAD.to_owned(),
            branch: "feature/verify".to_owned(),
            object_format: GitObjectFormat::Sha1,
            snapshot_name: "raw-git-1234-abcdef-7a-9".to_owned(),
        }
    }

    #[test]
    fn request_roundtrips_and_rejects_unknown_fields() {
        let request = sample_request();
        let json = serde_json::to_string(&request).expect("serialize");
        let decoded: GitVerifyRequest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, request);
        decoded.validate().expect("valid request");

        let with_unknown = json.replace('{', "{\"unexpected\":true,");
        assert!(serde_json::from_str::<GitVerifyRequest>(&with_unknown).is_err());
    }

    #[test]
    fn result_has_no_deserialize_and_roundtrips_as_serialized_evidence() {
        let result = GitVerifyResult {
            head: HEAD.to_owned(),
            branch: "main".to_owned(),
            base: None,
            object_format: GitObjectFormat::Sha1,
        };
        let json = serde_json::to_string(&result).expect("serialize result");
        assert!(json.contains("\"object_format\":\"sha1\""));
        // GitVerifyResult intentionally has no Deserialize implementation.
        assert!(json.contains("\"head\""));
    }

    #[test]
    fn runtime_availability_roundtrips_and_rejects_unknown_fields() {
        let configured = GitRuntimeAvailability {
            status: GitRuntimeStatus::ConfiguredUnverified,
            reason: None,
        };
        let json = serde_json::to_string(&configured).expect("serialize");
        assert_eq!(json, r#"{"status":"configured_unverified","reason":null}"#);
        assert_eq!(
            serde_json::from_str::<GitRuntimeAvailability>(&json).expect("deserialize"),
            configured
        );

        let invalid = GitRuntimeAvailability {
            status: GitRuntimeStatus::Invalid,
            reason: Some(GitRuntimeReason::PrivateRootSymlink),
        };
        let json = serde_json::to_string(&invalid).expect("serialize");
        assert!(json.contains("\"status\":\"invalid\""));
        assert!(json.contains("\"private_root_symlink\""));

        assert!(
            serde_json::from_str::<GitRuntimeAvailability>(
                r#"{"status":"invalid","reason":"private_root_symlink","path":"/secret"}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<GitRuntimeAvailability>(
                r#"{"status":"invalid","reason":"not_a_reason"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn object_formats_use_snake_case() {
        assert_eq!(
            serde_json::to_string(&GitObjectFormat::Sha1).expect("sha1"),
            "\"sha1\""
        );
        assert_eq!(
            serde_json::to_string(&GitObjectFormat::Sha256).expect("sha256"),
            "\"sha256\""
        );
        assert_eq!(GitObjectFormat::Sha1.hash_hex_len(), 40);
        assert_eq!(GitObjectFormat::Sha256.hash_hex_len(), 64);
    }

    #[test]
    fn structural_validation_rejects_wrong_owner_and_ids() {
        let mut request = sample_request();
        request.owner = RunnerOwner::InternalUser { user_id: 1 };
        assert_eq!(request.validate(), Err(GitVerifyRequestError::InvalidOwner));

        let mut request = sample_request();
        request.owner = owner(12);
        assert_eq!(request.validate(), Err(GitVerifyRequestError::InvalidOwner));

        let mut request = sample_request();
        request.workspace_binding_id = 0;
        assert_eq!(
            request.validate(),
            Err(GitVerifyRequestError::InvalidWorkspaceBinding)
        );

        let mut request = sample_request();
        request.code_project_id = Uuid::nil();
        assert_eq!(
            request.validate(),
            Err(GitVerifyRequestError::InvalidCodeProject)
        );

        let mut request = sample_request();
        request.authorization_generation = Uuid::nil();
        assert_eq!(
            request.validate(),
            Err(GitVerifyRequestError::InvalidGeneration)
        );
    }

    #[test]
    fn structural_validation_rejects_bad_heads_and_formats() {
        for head in [
            "".to_owned(),
            "abc".to_owned(),
            "A".repeat(40),
            "g".repeat(40),
            "a".repeat(41),
        ] {
            let mut request = sample_request();
            request.expected_new_head = head;
            assert_eq!(request.validate(), Err(GitVerifyRequestError::InvalidHead));
        }

        let mut request = sample_request();
        request.object_format = GitObjectFormat::Sha256;
        assert_eq!(request.validate(), Err(GitVerifyRequestError::InvalidHead));

        let mut request = sample_request();
        request.expected_new_head = "a".repeat(64);
        request.object_format = GitObjectFormat::Sha256;
        request.validate().expect("sha256 head");

        let mut request = sample_request();
        request.expected_base = Some(OTHER.to_owned());
        request.validate().expect("valid base");

        let mut request = sample_request();
        request.expected_base = Some("A".repeat(40));
        assert_eq!(request.validate(), Err(GitVerifyRequestError::InvalidBase));
    }

    #[test]
    fn structural_validation_rejects_bad_branches() {
        for branch in [
            "",
            "main\n",
            "../main",
            "main/../x",
            "/main",
            "main/",
            "-main",
            ".main",
            "main.",
            "main.lock",
            "main//x",
            "main~x",
            "main^x",
            "main:x",
            "main?x",
            "main*x",
            "main[x",
            "main\\x",
            "main x",
            "main$x",
            "main`x",
            "main;x",
            "main|x",
            "main&x",
            "main<x",
        ] {
            let mut request = sample_request();
            request.branch = branch.to_owned();
            assert_eq!(
                request.validate(),
                Err(GitVerifyRequestError::InvalidBranch),
                "branch {branch:?}"
            );
        }
    }

    #[test]
    fn structural_validation_rejects_paths_and_foreign_snapshot_names() {
        for name in [
            "",
            "../raw-git-1-a-b-0",
            "raw-git-1-a-b-0/child",
            "fixed-snapshot",
            "raw-git-a-a-b-0",
            "raw-git-1-A-b-0",
            "raw-git-1--b-0",
            "raw-git-1-a-b-x",
            "raw-git-1-a-b-0-extra",
            "/raw-git-1-a-b-0",
        ] {
            let mut request = sample_request();
            request.snapshot_name = name.to_owned();
            assert_eq!(
                request.validate(),
                Err(GitVerifyRequestError::InvalidSnapshotName),
                "snapshot {name:?}"
            );
        }
    }
    fn sample_spec() -> GitJobSpec {
        GitJobSpec {
            job_id: Uuid::new_v4(),
            operation_id: Uuid::new_v4(),
            owner: RunnerOwner::ApplicationPrincipal {
                application_id: 7,
                workspace_binding_id: 11,
                principal_digest: "a".repeat(64),
            },
            application_id: 7,
            workspace_binding_id: 11,
            principal_digest: "a".repeat(64),
            code_project_id: Uuid::new_v4(),
            authorization_generation: Uuid::new_v4(),
            canonical_branch: "main".to_owned(),
            expected_base: None,
            reserved_bytes: 1024,
            deadline_unix_ms: 1_800_000_000_000,
            workspace_root: "/srv/workspaces/binding-11".to_owned(),
        }
    }

    #[test]
    fn git_job_spec_roundtrips_and_rejects_unknown_or_inconsistent_input() {
        let spec = sample_spec();
        let json = serde_json::to_string(&spec).expect("serialize spec");
        assert!(!json.contains("token"));
        assert!(!json.contains("password"));
        let decoded: GitJobSpec = serde_json::from_str(&json).expect("deserialize spec");
        assert_eq!(decoded, spec);
        decoded.validate().expect("valid spec");

        let unknown = json.replace('{', "{\"credential\":\"canary\",");
        assert!(serde_json::from_str::<GitJobSpec>(&unknown).is_err());

        let mut bad = sample_spec();
        bad.owner = RunnerOwner::InternalUser { user_id: 1 };
        assert_eq!(bad.validate(), Err(GitJobSpecError::InvalidOwner));

        let mut bad = sample_spec();
        bad.workspace_root = "relative/path".to_owned();
        assert_eq!(bad.validate(), Err(GitJobSpecError::InvalidWorkspaceRoot));

        let mut bad = sample_spec();
        bad.reserved_bytes = 0;
        assert_eq!(bad.validate(), Err(GitJobSpecError::InvalidReservation));

        let mut bad = sample_spec();
        bad.principal_digest = "A".repeat(64);
        assert_eq!(bad.validate(), Err(GitJobSpecError::InvalidPrincipal));
    }

    #[test]
    fn git_job_outcome_roundtrips_and_rejects_unknown_or_bad_heads() {
        let success = GitJobOutcome::SyncSucceeded {
            operation_id: Uuid::new_v4(),
            head: HEAD.to_owned(),
            base: Some(OTHER.to_owned()),
            published: true,
            cleanup_confirmed: true,
        };
        let json = serde_json::to_string(&success).expect("serialize outcome");
        assert!(json.contains("\"outcome\":\"sync_succeeded\""));
        assert_eq!(
            serde_json::from_str::<GitJobOutcome>(&json).expect("deserialize outcome"),
            success
        );
        success.validate().expect("valid outcome");

        let failed = GitJobOutcome::Failed {
            operation_id: Uuid::new_v4(),
            error_code: GitJobErrorCode::CredentialUnavailable,
            cleanup_confirmed: false,
        };
        let json = serde_json::to_string(&failed).expect("serialize failed outcome");
        assert!(json.contains("\"error_code\":\"credential_unavailable\""));
        failed.validate().expect("valid failed outcome");

        let unknown = serde_json::to_string(&success)
            .expect("serialize")
            .replace('{', "{\"remote_url\":\"https://canary.invalid/\",");
        assert!(serde_json::from_str::<GitJobOutcome>(&unknown).is_err());

        let mut bad = success.clone();
        bad = match bad {
            GitJobOutcome::SyncSucceeded {
                operation_id,
                base,
                published,
                ..
            } => GitJobOutcome::SyncSucceeded {
                operation_id,
                head: "A".repeat(40),
                base,
                published,
                cleanup_confirmed: true,
            },
            other => other,
        };
        assert_eq!(bad.validate(), Err(GitJobOutcomeError::InvalidHead));

        let bad_base = match success.clone() {
            GitJobOutcome::SyncSucceeded {
                operation_id,
                head,
                published,
                ..
            } => GitJobOutcome::SyncSucceeded {
                operation_id,
                head,
                base: Some("A".repeat(40)),
                published,
                cleanup_confirmed: true,
            },
            other => other,
        };
        assert_eq!(bad_base.validate(), Err(GitJobOutcomeError::InvalidBase));

        let unpublished = match success.clone() {
            GitJobOutcome::SyncSucceeded {
                operation_id,
                head,
                base,
                ..
            } => GitJobOutcome::SyncSucceeded {
                operation_id,
                head,
                base,
                published: false,
                cleanup_confirmed: true,
            },
            other => other,
        };
        assert_eq!(unpublished.validate(), Err(GitJobOutcomeError::Unpublished));

        let success_json = serde_json::to_string(&success).expect("serialize");
        let missing_published = success_json.replace(",\"published\":true", "");
        assert!(
            serde_json::from_str::<GitJobOutcome>(&missing_published).is_err(),
            "published is required, never defaulted"
        );
        let missing_base =
            success_json.replace("\"base\":\"fedcba9876543210fedcba9876543210fedcba98\",", "");
        assert!(
            serde_json::from_str::<GitJobOutcome>(&missing_base).is_err(),
            "base must be present even when null"
        );
        let null_base = success_json.replace(
            "\"base\":\"fedcba9876543210fedcba9876543210fedcba98\"",
            "\"base\":null",
        );
        let decoded: GitJobOutcome =
            serde_json::from_str(&null_base).expect("null base is a first clone");
        decoded.validate().expect("first clone outcome");
        let bad_base = success_json.replace(
            "\"base\":\"fedcba9876543210fedcba9876543210fedcba98\"",
            "\"base\":\"ABC\"",
        );
        let bad_base = serde_json::from_str::<GitJobOutcome>(&bad_base)
            .expect("malformed base is still structurally typed");
        assert_eq!(bad_base.validate(), Err(GitJobOutcomeError::InvalidBase));
    }

    fn sample_read_request() -> GitReceiptReadRequest {
        GitReceiptReadRequest {
            job_id: Uuid::new_v4(),
            operation_id: Uuid::new_v4(),
            assigned_manager_id: 9,
            owner: owner(11),
            canonical_branch: "main".to_owned(),
            expected_base: Some(OTHER.to_owned()),
            workspace_root: "/srv/workspaces/binding-11".to_owned(),
            deadline_unix_ms: 1_800_000_000_000,
        }
    }

    fn confirmed_ack(request: &GitReceiptReadRequest) -> GitReceiptReadAck {
        GitReceiptReadAck {
            job_id: request.job_id,
            operation_id: request.operation_id,
            workspace_binding_id: request.workspace_binding_id().expect("binding"),
            state: GitReceiptState::Confirmed,
            phase: Some(GitReceiptPhase::Completed),
            base: request.expected_base.clone(),
            head: Some(HEAD.to_owned()),
            published: true,
            cleanup_confirmed: true,
        }
    }

    #[test]
    fn receipt_read_request_roundtrips_and_rejects_unknown_or_mismatched_identity() {
        let request = sample_read_request();
        let json = serde_json::to_string(&request).expect("serialize request");
        assert_eq!(json.matches("workspace_binding_id").count(), 1);
        let decoded: GitReceiptReadRequest =
            serde_json::from_str(&json).expect("deserialize request");
        assert_eq!(decoded, request);
        decoded.validate().expect("valid request");
        assert_eq!(decoded.workspace_binding_id(), Some(11));

        let unknown = json.replace('{', "{\"path\":\"/tmp/receipt\",");
        assert!(serde_json::from_str::<GitReceiptReadRequest>(&unknown).is_err());
        let missing_base = json.replace(
            "\"expected_base\":\"fedcba9876543210fedcba9876543210fedcba98\",",
            "",
        );
        assert!(
            serde_json::from_str::<GitReceiptReadRequest>(&missing_base).is_err(),
            "expected_base must be present even when null"
        );
        let missing_root = json.replace("\"workspace_root\":\"/srv/workspaces/binding-11\",", "");
        assert!(
            serde_json::from_str::<GitReceiptReadRequest>(&missing_root).is_err(),
            "workspace_root is required"
        );
        let mut bad = sample_read_request();
        bad.workspace_root = "relative/root".to_owned();
        assert_eq!(
            bad.validate(),
            Err(GitReceiptReadRequestError::InvalidWorkspaceRoot)
        );

        let mut bad = sample_read_request();
        bad.assigned_manager_id = 0;
        assert_eq!(
            bad.validate(),
            Err(GitReceiptReadRequestError::InvalidManager)
        );
        let mut bad = sample_read_request();
        bad.owner = RunnerOwner::WorkspaceBinding {
            workspace_binding_id: 11,
        };
        assert_eq!(
            bad.validate(),
            Err(GitReceiptReadRequestError::InvalidOwner)
        );
        let mut bad = sample_read_request();
        bad.canonical_branch = "../main".to_owned();
        assert_eq!(
            bad.validate(),
            Err(GitReceiptReadRequestError::InvalidBranch)
        );
        let mut bad = sample_read_request();
        bad.expected_base = Some("A".repeat(40));
        assert_eq!(bad.validate(), Err(GitReceiptReadRequestError::InvalidBase));
        let mut bad = sample_read_request();
        bad.deadline_unix_ms = 0;
        assert_eq!(
            bad.validate(),
            Err(GitReceiptReadRequestError::InvalidDeadline)
        );
    }

    #[test]
    fn receipt_read_ack_is_strict_and_only_a_confirmed_completed_ack_promotes() {
        let request = sample_read_request();
        let ack = confirmed_ack(&request);
        let json = serde_json::to_string(&ack).expect("serialize ack");
        assert_eq!(
            serde_json::from_str::<GitReceiptReadAck>(&json).expect("deserialize ack"),
            ack
        );
        ack.validate().expect("valid ack");
        ack.validate_matches(&request).expect("identity match");
        assert!(ack.confirms(&request));

        let unknown = json.replace('{', "{\"receipt_path\":\"/tmp/receipt\",");
        assert!(serde_json::from_str::<GitReceiptReadAck>(&unknown).is_err());
        let missing_base =
            json.replace("\"base\":\"fedcba9876543210fedcba9876543210fedcba98\",", "");
        assert!(
            serde_json::from_str::<GitReceiptReadAck>(&missing_base).is_err(),
            "ack base must be present even when null"
        );

        let mut missing = ack.clone();
        missing.state = GitReceiptState::Missing;
        missing.phase = None;
        missing.head = None;
        missing.published = false;
        missing.cleanup_confirmed = false;
        missing.validate().expect("valid missing ack");
        assert!(!missing.confirms(&request), "missing never promotes");

        let mut unconfirmed = ack.clone();
        unconfirmed.state = GitReceiptState::Unconfirmed;
        assert!(
            !unconfirmed.confirms(&request),
            "unconfirmed never promotes"
        );

        let mut wrong_phase = ack.clone();
        wrong_phase.phase = Some(GitReceiptPhase::Verified);
        assert_eq!(
            wrong_phase.validate(),
            Err(GitReceiptReadAckError::Unconfirmed)
        );
        assert!(!wrong_phase.confirms(&request));

        let mut unpublished = ack.clone();
        unpublished.published = false;
        assert_eq!(
            unpublished.validate(),
            Err(GitReceiptReadAckError::Unconfirmed)
        );

        let mut dirty = ack.clone();
        dirty.cleanup_confirmed = false;
        assert_eq!(dirty.validate(), Err(GitReceiptReadAckError::Unconfirmed));

        let mut no_head = ack.clone();
        no_head.head = None;
        assert_eq!(no_head.validate(), Err(GitReceiptReadAckError::Unconfirmed));

        let mut other_job = ack.clone();
        other_job.job_id = Uuid::new_v4();
        other_job.validate().expect("shape is valid");
        assert_eq!(
            other_job.validate_matches(&request),
            Err(GitReceiptReadAckError::InvalidIdentity)
        );
        assert!(!other_job.confirms(&request));

        let mut other_binding = ack.clone();
        other_binding.workspace_binding_id = 12;
        assert_eq!(
            other_binding.validate_matches(&request),
            Err(GitReceiptReadAckError::InvalidIdentity)
        );

        let mut bad_head = ack;
        bad_head.head = Some("A".repeat(40));
        assert_eq!(
            bad_head.validate(),
            Err(GitReceiptReadAckError::InvalidHead)
        );
    }

    #[test]
    fn git_job_credential_debug_is_redacted_and_fields_are_strict() {
        let credential = GitJobCredential {
            username: "broker".to_owned(),
            password: format!("smcpw1.{}.{}", Uuid::new_v4(), "b".repeat(64)),
            remote_url: "https://git.example.invalid/api/internal/code-projects/ignored/git"
                .to_owned(),
            deadline_unix_ms: 1_800_000_000_000,
        };
        assert_eq!(
            format!("{credential:?}"),
            "GitJobCredential(..)",
            "Debug must not leak either secret field"
        );
        let json = serde_json::to_string(&credential).expect("serialize credential");
        assert_eq!(
            serde_json::from_str::<GitJobCredential>(&json).expect("deserialize credential"),
            credential
        );
        let unknown = json.replace('{', "{\"mac\":\"canary\",");
        assert!(serde_json::from_str::<GitJobCredential>(&unknown).is_err());
    }
}
