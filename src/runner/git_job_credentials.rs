//! Transient, in-memory Git credentials bound to one claimed job.
//!
//! The store is the only place a broker secret lives inside the process. It is
//! never persisted, logged, or placed in a job payload. Entries are immutable
//! identities: the same job id cannot be overwritten with a different scope,
//! and delivery is idempotent while the claimed row and entry remain live.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::Url;
use runner_protocol::{
    GIT_JOB_CREDENTIAL_URL_MAX_LEN, GIT_JOB_CREDENTIAL_USERNAME, GitJobCredential,
    is_canonical_branch,
};
use uuid::Uuid;

/// Hard entry ceiling. Expired entries are pruned before this is enforced.
pub const GIT_JOB_CREDENTIAL_CAPACITY: usize = 64;

/// Full immutable scope stored with one credential.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitJobCredentialIdentity {
    pub job_id: Uuid,
    pub operation_id: Uuid,
    pub assigned_manager_id: i64,
    pub application_id: i64,
    pub workspace_binding_id: i64,
    pub principal_digest: String,
    pub code_project_id: Uuid,
    pub authorization_generation: Uuid,
    pub canonical_branch: String,
    pub deadline_unix_ms: i64,
}

/// One credential plus the exact job scope it belongs to.
#[derive(Clone)]
pub struct GitJobCredentialEntry {
    pub identity: GitJobCredentialIdentity,
    pub credential: GitJobCredential,
}

impl fmt::Debug for GitJobCredentialEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GitJobCredentialEntry")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitJobCredentialStoreError {
    InvalidIdentity,
    InvalidCredential,
    IdentityConflict,
    Full,
}

impl fmt::Display for GitJobCredentialStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidIdentity => "git job credential identity is invalid",
            Self::InvalidCredential => "git job credential is invalid",
            Self::IdentityConflict => "git job credential identity already exists",
            Self::Full => "git job credential store is full",
        })
    }
}

impl std::error::Error for GitJobCredentialStoreError {}

/// Bounded, redacted map from job id to one transient credential.
pub struct GitJobCredentialStore {
    entries: Mutex<HashMap<Uuid, GitJobCredentialEntry>>,
}

impl Default for GitJobCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

impl GitJobCredentialStore {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Insert one fully validated entry.
    ///
    /// Expired entries are pruned first. An existing `job_id` may only be
    /// reinserted with the exact same identity and credential; anything else is
    /// an identity conflict. A live full store refuses rather than evicting.
    pub fn insert(&self, entry: GitJobCredentialEntry) -> Result<(), GitJobCredentialStoreError> {
        validate_identity(&entry.identity)?;
        if entry.identity.deadline_unix_ms != entry.credential.deadline_unix_ms {
            return Err(GitJobCredentialStoreError::InvalidCredential);
        }
        if !validate_git_job_credential(&entry.credential, entry.identity.code_project_id) {
            return Err(GitJobCredentialStoreError::InvalidCredential);
        }

        let mut entries = self.entries.lock().expect("git job credential store lock");
        prune_expired_locked(&mut entries, unix_ms_now());
        if let Some(existing) = entries.get(&entry.identity.job_id) {
            if existing.identity == entry.identity && existing.credential == entry.credential {
                return Ok(());
            }
            return Err(GitJobCredentialStoreError::IdentityConflict);
        }
        if entries.len() >= GIT_JOB_CREDENTIAL_CAPACITY {
            return Err(GitJobCredentialStoreError::Full);
        }
        entries.insert(entry.identity.job_id, entry);
        Ok(())
    }

    /// Return the credential only when the full live identity matches.
    pub fn get(&self, identity: &GitJobCredentialIdentity) -> Option<GitJobCredential> {
        if validate_identity(identity).is_err() {
            return None;
        }
        let mut entries = self.entries.lock().expect("git job credential store lock");
        let now = unix_ms_now();
        prune_expired_locked(&mut entries, now);
        entries
            .get(&identity.job_id)
            .filter(|entry| entry.identity == *identity && identity.deadline_unix_ms > now)
            .map(|entry| entry.credential.clone())
    }

    /// Remove the entry only when the full live identity matches.
    pub fn remove(&self, identity: &GitJobCredentialIdentity) -> Option<GitJobCredential> {
        if validate_identity(identity).is_err() {
            return None;
        }
        let mut entries = self.entries.lock().expect("git job credential store lock");
        prune_expired_locked(&mut entries, unix_ms_now());
        if entries
            .get(&identity.job_id)
            .is_some_and(|entry| entry.identity == *identity)
        {
            return entries
                .remove(&identity.job_id)
                .map(|entry| entry.credential);
        }
        None
    }

    /// Drop entries whose deadline has passed. Returns the number removed.
    pub fn prune_expired(&self) -> usize {
        let mut entries = self.entries.lock().expect("git job credential store lock");
        prune_expired_locked(&mut entries, unix_ms_now())
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries
            .lock()
            .expect("git job credential store lock")
            .len()
    }
}

impl fmt::Debug for GitJobCredentialStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GitJobCredentialStore(..)")
    }
}

fn prune_expired_locked(entries: &mut HashMap<Uuid, GitJobCredentialEntry>, now: i64) -> usize {
    let before = entries.len();
    entries.retain(|_, entry| entry.identity.deadline_unix_ms > now);
    before - entries.len()
}

fn validate_identity(
    identity: &GitJobCredentialIdentity,
) -> Result<(), GitJobCredentialStoreError> {
    if identity.job_id.is_nil()
        || identity.operation_id.is_nil()
        || identity.code_project_id.is_nil()
        || identity.authorization_generation.is_nil()
        || identity.assigned_manager_id <= 0
        || identity.application_id <= 0
        || identity.workspace_binding_id <= 0
        || identity.deadline_unix_ms <= 0
        || !is_lower_hex_exact(&identity.principal_digest, 64)
        || !is_canonical_branch(&identity.canonical_branch)
    {
        return Err(GitJobCredentialStoreError::InvalidIdentity);
    }
    Ok(())
}

#[derive(Clone, Eq, PartialEq)]
pub struct SplitScopedGitUrl {
    pub remote_url: String,
    pub password: String,
}

impl std::fmt::Debug for SplitScopedGitUrl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SplitScopedGitUrl")
            .field("remote_url", &self.remote_url)
            .field("password", &"[redacted]")
            .finish()
    }
}

/// Split one bounded scoped broker URL into a credential-free remote URL and
/// the scoped password. The password never enters the returned URL.
pub fn split_scoped_git_url(input: &str, code_project_id: Uuid) -> Option<SplitScopedGitUrl> {
    let url = parse_scoped_url(input, code_project_id)?;
    let password = url.password()?.to_owned();
    if !validate_scoped_password(&password) {
        return None;
    }
    let mut remote = url.clone();
    remote.set_username("").ok()?;
    remote.set_password(None).ok()?;
    if !remote.username().is_empty() || remote.password().is_some() {
        return None;
    }
    Some(SplitScopedGitUrl {
        remote_url: remote.to_string(),
        password,
    })
}

fn parse_scoped_url(input: &str, code_project_id: Uuid) -> Option<Url> {
    let url = parse_bounded_url(input, true)?;
    if url.username() != GIT_JOB_CREDENTIAL_USERNAME
        || url.password().is_none()
        || url.path() != project_git_path(code_project_id)
    {
        return None;
    }
    Some(url)
}

fn parse_bounded_url(input: &str, expect_userinfo: bool) -> Option<Url> {
    if input.is_empty()
        || input.len() > GIT_JOB_CREDENTIAL_URL_MAX_LEN
        || input.contains('#')
        || input.contains('%')
        || input.contains('\\')
    {
        return None;
    }
    let authority_start = input.find("://")? + 3;
    let authority = input[authority_start..].split('/').next()?;
    let at_count = authority.matches('@').count();
    if (expect_userinfo && at_count != 1) || (!expect_userinfo && at_count != 0) {
        return None;
    }
    let rest = &input[authority_start..];
    let (_, raw_path) = rest.split_once('/')?;
    if raw_path.is_empty()
        || raw_path.starts_with('/')
        || raw_path.contains("..")
        || raw_path.contains("//")
        || raw_path.contains('\\')
    {
        return None;
    }
    let url = Url::parse(input).ok()?;
    if url.scheme() != "https"
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port() == Some(0)
    {
        return None;
    }
    let host = url.host_str()?;
    if host.is_empty() || host.len() > GIT_JOB_CREDENTIAL_URL_MAX_LEN {
        return None;
    }
    Some(url)
}

fn project_git_path(code_project_id: Uuid) -> String {
    format!("/api/internal/code-projects/{code_project_id}/git")
}

/// Validate the fixed broker username, scoped password syntax, and exact
/// credential-free URL. Cryptographic authority stays with the broker.
pub fn validate_git_job_credential(credential: &GitJobCredential, code_project_id: Uuid) -> bool {
    credential.username == GIT_JOB_CREDENTIAL_USERNAME
        && validate_scoped_password(&credential.password)
        && credential.deadline_unix_ms > 0
        && validate_remote_url(&credential.remote_url, code_project_id)
}

fn validate_scoped_password(password: &str) -> bool {
    if password.len() != 7 + 36 + 1 + 64 {
        return false;
    }
    let mut parts = password.split('.');
    let (Some("smcpw1"), Some(scope), Some(mac), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let Ok(uuid) = Uuid::parse_str(scope) else {
        return false;
    };
    uuid.to_string() == scope && is_lower_hex_exact(mac, 64)
}

fn validate_remote_url(url: &str, code_project_id: Uuid) -> bool {
    let Some(parsed) = parse_bounded_url(url, false) else {
        return false;
    };
    parsed.scheme() == "https"
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && parsed.host_str().is_some_and(|host| !host.is_empty())
        && parsed.path() == project_git_path(code_project_id)
}

fn is_lower_hex_exact(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn unix_ms_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credential(code_project_id: Uuid, deadline_unix_ms: i64) -> GitJobCredential {
        GitJobCredential {
            username: "broker".to_owned(),
            password: format!("smcpw1.{}.{}", Uuid::new_v4(), "a".repeat(64)),
            remote_url: format!(
                "https://git.example.invalid/api/internal/code-projects/{code_project_id}/git"
            ),
            deadline_unix_ms,
        }
    }

    fn identity(code_project_id: Uuid, deadline_unix_ms: i64) -> GitJobCredentialIdentity {
        GitJobCredentialIdentity {
            job_id: Uuid::new_v4(),
            operation_id: Uuid::new_v4(),
            assigned_manager_id: 1,
            application_id: 2,
            workspace_binding_id: 3,
            principal_digest: "b".repeat(64),
            code_project_id,
            authorization_generation: Uuid::new_v4(),
            canonical_branch: "main".to_owned(),
            deadline_unix_ms,
        }
    }

    fn entry(deadline_unix_ms: i64) -> GitJobCredentialEntry {
        let project = Uuid::new_v4();
        GitJobCredentialEntry {
            identity: identity(project, deadline_unix_ms),
            credential: credential(project, deadline_unix_ms),
        }
    }

    #[test]
    fn store_is_bounded_prunes_and_never_evicts_live_entries() {
        let store = GitJobCredentialStore::new();
        let future = unix_ms_now() + 60_000;
        for _ in 0..GIT_JOB_CREDENTIAL_CAPACITY {
            store.insert(entry(future)).expect("insert below cap");
        }
        assert_eq!(store.len(), GIT_JOB_CREDENTIAL_CAPACITY);
        assert_eq!(
            store.insert(entry(future)),
            Err(GitJobCredentialStoreError::Full)
        );
        assert_eq!(store.len(), GIT_JOB_CREDENTIAL_CAPACITY);

        let short = GitJobCredentialStore::new();
        short
            .insert(entry(unix_ms_now() - 1))
            .expect("expired insert");
        assert_eq!(short.prune_expired(), 1);
        assert_eq!(short.len(), 0);
    }

    #[test]
    fn identity_conflicts_are_rejected_and_identical_reinsert_is_idempotent() {
        let store = GitJobCredentialStore::new();
        let entry = entry(unix_ms_now() + 60_000);
        store.insert(entry.clone()).expect("first insert");
        store.insert(entry.clone()).expect("identical reinsert");
        assert_eq!(
            store.get(&entry.identity).expect("live credential"),
            entry.credential
        );

        let mut changed = entry.clone();
        changed.identity.operation_id = Uuid::new_v4();
        assert_eq!(
            store.insert(changed),
            Err(GitJobCredentialStoreError::IdentityConflict)
        );
        assert_eq!(
            store.get(&entry.identity).expect("original retained"),
            entry.credential
        );
    }

    #[test]
    fn get_and_remove_require_the_full_identity_and_expiry() {
        let store = GitJobCredentialStore::new();
        let entry = entry(unix_ms_now() + 60_000);
        store.insert(entry.clone()).expect("insert");
        let mut foreign = entry.identity.clone();
        foreign.assigned_manager_id += 1;
        assert!(store.get(&foreign).is_none());
        assert!(store.remove(&foreign).is_none());
        assert_eq!(
            store.remove(&entry.identity).expect("matching remove"),
            entry.credential
        );
        assert!(store.get(&entry.identity).is_none());
    }

    #[test]
    fn scoped_url_parser_uses_canonical_url_semantics() {
        let project = Uuid::new_v4();
        let password = format!("smcpw1.{}.{}", Uuid::new_v4(), "a".repeat(64));
        let canonical = format!(
            "https://broker:{password}@BROKER.example.invalid:443/api/internal/code-projects/{project}/git"
        );
        let split = split_scoped_git_url(&canonical, project).expect("canonical");
        assert_eq!(split.password, password);
        assert!(!split.remote_url.contains("broker:"));
        assert!(!format!("{split:?}").contains(&password));
        assert_eq!(
            split.remote_url,
            format!("https://broker.example.invalid/api/internal/code-projects/{project}/git")
        );

        let ipv6 =
            format!("https://broker:{password}@[::1]:443/api/internal/code-projects/{project}/git");
        let split = split_scoped_git_url(&ipv6, project).expect("ipv6");
        assert_eq!(
            split.remote_url,
            format!("https://[::1]/api/internal/code-projects/{project}/git")
        );

        let bad = [
            format!("http://broker:{password}@host/api/internal/code-projects/{project}/git"),
            format!("https://user:{password}@host/api/internal/code-projects/{project}/git"),
            format!("https://broker:{password}@host:abc/api/internal/code-projects/{project}/git"),
            format!(
                "https://broker:{password}@host:65536/api/internal/code-projects/{project}/git"
            ),
            format!("https://broker:{password}@host:0/api/internal/code-projects/{project}/git"),
            format!("https://broker:{password}@host/api/internal/code-projects/{project}/git?x=1"),
            format!("https://broker:{password}@host/api/internal/code-projects/{project}/git#x"),
            format!(
                "https://broker:{password}@host/api/internal/code-projects/{project}/git/../git"
            ),
            format!("https://broker:{password}@host/api/internal/code-projects/{project}/other"),
            format!("https://broker:{password}@host//api/internal/code-projects/{project}/git"),
            format!("https://broker:{password}@host/api%2Finternal/code-projects/{project}/git"),
            format!("https://broker:{password}@ho%73t/api/internal/code-projects/{project}/git"),
            format!("https://bro%6ber:{password}@host/api/internal/code-projects/{project}/git"),
            format!(
                "https://broker:%73mcpw1.{}.{}@host/api/internal/code-projects/{project}/git",
                Uuid::new_v4(),
                "a".repeat(64)
            ),
            format!("https://broker:{password}@host/api/internal/code-projects/{project}/git\\"),
            format!("https://a@b@host/api/internal/code-projects/{project}/git"),
            format!(
                "https://broker:{password}@host/api/internal/code-projects/{}/git",
                Uuid::new_v4()
            ),
        ];
        for value in bad {
            assert!(split_scoped_git_url(&value, project).is_none(), "{value}");
        }

        assert!(validate_remote_url(
            &format!("https://host/api/internal/code-projects/{project}/git"),
            project
        ));
        for value in [
            format!("https://user:pass@host/api/internal/code-projects/{project}/git"),
            format!("https://host/api/internal/code-projects/{project}/git?x=1"),
            format!("https://host/api/internal/code-projects/{project}/git#x"),
            format!("https://host:0/api/internal/code-projects/{project}/git"),
            format!("https://host/api/internal/code-projects/{project}/git/../git"),
            format!(
                "https://host/api/internal/code-projects/{}/git",
                Uuid::new_v4()
            ),
        ] {
            assert!(!validate_remote_url(&value, project), "{value}");
        }
    }

    #[test]
    fn credential_syntax_is_closed() {
        let project = Uuid::new_v4();
        assert!(validate_git_job_credential(
            &credential(project, unix_ms_now() + 1),
            project
        ));
        for bad in [
            credential(Uuid::new_v4(), unix_ms_now() + 1),
            GitJobCredential {
                username: "other".to_owned(),
                ..credential(project, unix_ms_now() + 1)
            },
            GitJobCredential {
                password: "smcpw1.not-a-uuid".to_owned(),
                ..credential(project, unix_ms_now() + 1)
            },
            GitJobCredential {
                password: format!("smcpw1.{}.{}", Uuid::new_v4(), "A".repeat(64)),
                ..credential(project, unix_ms_now() + 1)
            },
            GitJobCredential {
                remote_url: format!(
                    "https://user:pass@git.example.invalid/api/internal/code-projects/{project}/git"
                ),
                ..credential(project, unix_ms_now() + 1)
            },
            GitJobCredential {
                remote_url: format!(
                    "https://git.example.invalid/api/internal/code-projects/{project}/git?x=1"
                ),
                ..credential(project, unix_ms_now() + 1)
            },
            GitJobCredential {
                remote_url: format!(
                    "https://git.example.invalid/api/internal/code-projects/{project}/other"
                ),
                ..credential(project, unix_ms_now() + 1)
            },
            GitJobCredential {
                remote_url: format!(
                    "http://git.example.invalid/api/internal/code-projects/{project}/git"
                ),
                ..credential(project, unix_ms_now() + 1)
            },
        ] {
            assert!(!validate_git_job_credential(&bad, project));
        }
    }

    #[test]
    fn debug_output_is_redacted() {
        let store = GitJobCredentialStore::new();
        let entry = entry(unix_ms_now() + 60_000);
        let password = entry.credential.password.clone();
        store.insert(entry).expect("insert");
        let rendered = format!("{store:?}");
        assert_eq!(rendered, "GitJobCredentialStore(..)");
        assert!(!rendered.contains(&password));
    }
}
