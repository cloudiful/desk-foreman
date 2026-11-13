//! Metadata-first workspace Git upstream management (issue 1180 P27/P39,
//! issue 18 P1).
//!
//! Every mutation locks the owning binding first so archived or resetting
//! bindings can never gain a new upstream. A stored credential is a single
//! authenticated ciphertext frame bound to the row that holds it; the
//! credential is written inside the same transaction that inserts or updates
//! the row. The plaintext secret never reaches the database: it is sealed
//! before the write and opened only to compare an unchanged credential.

use ring::constant_time::verify_slices_are_equal;
use sqlx::{PgPool, Postgres, Transaction};

use crate::{
    actor::check_write_lease,
    credential_cipher::{CredentialCipherError, CredentialProtector},
    db::types::{Page, WorkspaceBindingResponse, WorkspaceGitUpstreamResponse},
};

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

#[derive(Debug, thiserror::Error)]
pub enum GitUpstreamError {
    #[error("workspace binding not found")]
    BindingNotFound,
    #[error("workspace git upstream not found")]
    NotFound,
    #[error("workspace binding is not active")]
    BindingInactive,
    #[error("remote alias already exists for this workspace binding")]
    DuplicateAlias,
    #[error("application authority does not own this workspace binding")]
    Forbidden,
    #[error("workspace write lease is missing, stale or held by another owner")]
    LeaseConflict,
    #[error("credential encryption key is not configured")]
    CredentialKeyUnavailable,
    #[error("stored credential could not be decrypted")]
    CredentialUnreadable,
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for GitUpstreamError {
    fn from(error: sqlx::Error) -> Self {
        if is_unique_violation(&error) {
            Self::DuplicateAlias
        } else {
            Self::Internal(error.into())
        }
    }
}

pub struct NewWorkspaceGitUpstream<'a> {
    pub remote_alias: &'a str,
    pub target_url: &'a str,
    pub credential: Option<(&'a str, &'a str)>,
}

pub struct WorkspaceGitUpstreamUpdate<'a> {
    pub remote_alias: Option<&'a str>,
    pub target_url: Option<&'a str>,
    pub credential: Option<(&'a str, &'a str)>,
    pub clear_credential: bool,
}

/// Mutation authority validated while the binding row is locked inside the
/// store transaction, never from a pre-lock snapshot.
pub enum MutationAuthority<'a> {
    /// Existing admin-cookie callers. Admin authorization happened at the
    /// handler boundary and store semantics are unchanged.
    Admin,
    /// Internal application caller: exact application, exact binding subject
    /// and the current write lease must all still hold under the row lock.
    ApplicationSubject {
        application_id: i64,
        external_user_id: &'a str,
        lease_owner: &'a str,
    },
}

pub async fn list_workspace_git_upstreams(
    pool: &PgPool,
    workspace_binding_id: i64,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Page<WorkspaceGitUpstreamResponse>, GitUpstreamError> {
    ensure_binding_exists(pool, workspace_binding_id).await?;
    let (limit, offset) = bounded_page(limit, offset);
    let items = sqlx::query_as::<_, WorkspaceGitUpstreamResponse>(include_str!(
        "../sql/workspace_git_upstreams/list.sql"
    ))
    .bind(workspace_binding_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let total =
        sqlx::query_scalar::<_, i64>(include_str!("../sql/workspace_git_upstreams/count.sql"))
            .bind(workspace_binding_id)
            .fetch_one(pool)
            .await?;
    Ok(Page {
        items,
        total,
        limit,
        offset,
    })
}

pub async fn create_workspace_git_upstream(
    pool: &PgPool,
    protector: Option<&CredentialProtector>,
    workspace_binding_id: i64,
    input: NewWorkspaceGitUpstream<'_>,
) -> Result<WorkspaceGitUpstreamResponse, GitUpstreamError> {
    let mut tx = pool.begin().await?;
    lock_active_binding(&mut tx, workspace_binding_id).await?;
    let upstream = create_in_tx(&mut tx, protector, workspace_binding_id, input).await?;
    tx.commit().await?;
    Ok(upstream)
}

/// Application-scoped create: ownership, subject and the live write lease are
/// checked under the binding row lock inside the same transaction as the
/// mutation.
pub async fn create_workspace_git_upstream_for_application(
    pool: &PgPool,
    protector: Option<&CredentialProtector>,
    authority: &MutationAuthority<'_>,
    workspace_binding_id: i64,
    input: NewWorkspaceGitUpstream<'_>,
) -> Result<WorkspaceGitUpstreamResponse, GitUpstreamError> {
    let mut tx = pool.begin().await?;
    lock_authority_binding(&mut tx, workspace_binding_id, authority).await?;
    let upstream = create_in_tx(&mut tx, protector, workspace_binding_id, input).await?;
    tx.commit().await?;
    Ok(upstream)
}

async fn create_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    protector: Option<&CredentialProtector>,
    workspace_binding_id: i64,
    input: NewWorkspaceGitUpstream<'_>,
) -> Result<WorkspaceGitUpstreamResponse, GitUpstreamError> {
    let upstream_id =
        sqlx::query_scalar::<_, i64>(include_str!("../sql/workspace_git_upstreams/insert.sql"))
            .bind(workspace_binding_id)
            .bind(input.remote_alias)
            .bind(input.target_url)
            .fetch_one(&mut **tx)
            .await?;

    if let Some((username, secret)) = input.credential {
        let protector = protector.ok_or(GitUpstreamError::CredentialKeyUnavailable)?;
        let ciphertext = protector
            .seal(workspace_binding_id, upstream_id, secret)
            .map_err(map_cipher_error)?;
        write_credential(tx, workspace_binding_id, upstream_id, username, &ciphertext).await?;
    }

    read_upstream(tx, workspace_binding_id, upstream_id).await
}

pub async fn update_workspace_git_upstream(
    pool: &PgPool,
    protector: Option<&CredentialProtector>,
    workspace_binding_id: i64,
    upstream_id: i64,
    input: WorkspaceGitUpstreamUpdate<'_>,
) -> Result<WorkspaceGitUpstreamResponse, GitUpstreamError> {
    let mut tx = pool.begin().await?;
    lock_active_binding(&mut tx, workspace_binding_id).await?;
    let upstream =
        update_in_tx(&mut tx, protector, workspace_binding_id, upstream_id, input).await?;
    tx.commit().await?;
    Ok(upstream)
}

pub async fn update_workspace_git_upstream_for_application(
    pool: &PgPool,
    protector: Option<&CredentialProtector>,
    authority: &MutationAuthority<'_>,
    workspace_binding_id: i64,
    upstream_id: i64,
    input: WorkspaceGitUpstreamUpdate<'_>,
) -> Result<WorkspaceGitUpstreamResponse, GitUpstreamError> {
    let mut tx = pool.begin().await?;
    lock_authority_binding(&mut tx, workspace_binding_id, authority).await?;
    let upstream =
        update_in_tx(&mut tx, protector, workspace_binding_id, upstream_id, input).await?;
    tx.commit().await?;
    Ok(upstream)
}

#[derive(sqlx::FromRow)]
struct CurrentUpstream {
    remote_alias: String,
    target_url: String,
    credential_username: Option<String>,
    credential_ciphertext: Option<Vec<u8>>,
}

#[derive(Debug)]
enum CredentialAction<'a> {
    /// Leave the stored credential untouched.
    Keep,
    /// Store a fresh encrypted credential for the supplied username.
    Replace(&'a str, &'a str),
    /// Explicitly clear the stored credential.
    Clear,
}

/// Resolve the credential mutation for an update.
///
/// A replacement is a no-op only when the stored username matches and the
/// stored secret compares equal in constant time. The comparison opens the
/// stored frame under the expected row identity, so a frame that belongs to
/// another row can never be read here. A stored credential that cannot be
/// opened fails closed: the row is never cleared or overwritten because of a
/// cryptographic failure. An explicit clear needs no key, and a row without a
/// stored credential needs no key either.
fn resolve_credential_action<'a>(
    current: &CurrentUpstream,
    protector: Option<&CredentialProtector>,
    workspace_binding_id: i64,
    upstream_id: i64,
    input: &WorkspaceGitUpstreamUpdate<'a>,
) -> Result<CredentialAction<'a>, GitUpstreamError> {
    if input.clear_credential {
        if current.credential_username.is_none() {
            return Ok(CredentialAction::Keep);
        }
        return Ok(CredentialAction::Clear);
    }

    let Some((username, secret)) = input.credential else {
        return Ok(CredentialAction::Keep);
    };

    if let Some(frame) = current.credential_ciphertext.as_ref() {
        let protector = protector.ok_or(GitUpstreamError::CredentialKeyUnavailable)?;
        let stored = protector
            .open(workspace_binding_id, upstream_id, frame)
            .map_err(map_cipher_error)?;
        let username_matches = current.credential_username.as_deref() == Some(username);
        if username_matches && verify_slices_are_equal(stored.as_bytes(), secret.as_bytes()).is_ok()
        {
            return Ok(CredentialAction::Keep);
        }
    }

    if protector.is_none() {
        return Err(GitUpstreamError::CredentialKeyUnavailable);
    }

    Ok(CredentialAction::Replace(username, secret))
}

fn map_cipher_error(error: CredentialCipherError) -> GitUpstreamError {
    match error {
        CredentialCipherError::KeyUnavailable => GitUpstreamError::CredentialKeyUnavailable,
        CredentialCipherError::Unreadable => GitUpstreamError::CredentialUnreadable,
    }
}

async fn update_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    protector: Option<&CredentialProtector>,
    workspace_binding_id: i64,
    upstream_id: i64,
    input: WorkspaceGitUpstreamUpdate<'_>,
) -> Result<WorkspaceGitUpstreamResponse, GitUpstreamError> {
    lock_upstream(tx, workspace_binding_id, upstream_id).await?;
    let current = sqlx::query_as::<_, CurrentUpstream>(include_str!(
        "../sql/workspace_git_upstreams/current_for_update.sql"
    ))
    .bind(workspace_binding_id)
    .bind(upstream_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(GitUpstreamError::NotFound)?;

    let alias_changed = input
        .remote_alias
        .is_some_and(|value| value != current.remote_alias.as_str());
    let target_changed = input
        .target_url
        .is_some_and(|value| value != current.target_url.as_str());
    let credential_action = resolve_credential_action(
        &current,
        protector,
        workspace_binding_id,
        upstream_id,
        &input,
    )?;

    if !alias_changed && !target_changed && matches!(credential_action, CredentialAction::Keep) {
        return read_upstream(tx, workspace_binding_id, upstream_id).await;
    }

    let (clear_credential, credential_username, credential_ciphertext) = match credential_action {
        CredentialAction::Keep => (false, None, None),
        CredentialAction::Clear => (true, None, None),
        CredentialAction::Replace(username, secret) => {
            let protector = protector.ok_or(GitUpstreamError::CredentialKeyUnavailable)?;
            let ciphertext = protector
                .seal(workspace_binding_id, upstream_id, secret)
                .map_err(map_cipher_error)?;
            (false, Some(username), Some(ciphertext))
        }
    };

    sqlx::query_as::<_, WorkspaceGitUpstreamResponse>(include_str!(
        "../sql/workspace_git_upstreams/update.sql"
    ))
    .bind(workspace_binding_id)
    .bind(upstream_id)
    .bind(input.remote_alias)
    .bind(input.target_url)
    .bind(clear_credential)
    .bind(credential_username)
    .bind(credential_ciphertext)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(GitUpstreamError::NotFound)
}

pub async fn delete_workspace_git_upstream(
    pool: &PgPool,
    workspace_binding_id: i64,
    upstream_id: i64,
) -> Result<(), GitUpstreamError> {
    let mut tx = pool.begin().await?;
    lock_active_binding(&mut tx, workspace_binding_id).await?;
    delete_in_tx(&mut tx, workspace_binding_id, upstream_id).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn delete_workspace_git_upstream_for_application(
    pool: &PgPool,
    authority: &MutationAuthority<'_>,
    workspace_binding_id: i64,
    upstream_id: i64,
) -> Result<(), GitUpstreamError> {
    let mut tx = pool.begin().await?;
    lock_authority_binding(&mut tx, workspace_binding_id, authority).await?;
    delete_in_tx(&mut tx, workspace_binding_id, upstream_id).await?;
    tx.commit().await?;
    Ok(())
}

async fn delete_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_binding_id: i64,
    upstream_id: i64,
) -> Result<(), GitUpstreamError> {
    sqlx::query_scalar::<_, i64>(include_str!("../sql/workspace_git_upstreams/delete.sql"))
        .bind(workspace_binding_id)
        .bind(upstream_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(GitUpstreamError::NotFound)?;
    Ok(())
}

pub async fn set_workspace_git_upstream_enabled(
    pool: &PgPool,
    workspace_binding_id: i64,
    upstream_id: i64,
    enabled: bool,
) -> Result<WorkspaceGitUpstreamResponse, GitUpstreamError> {
    let mut tx = pool.begin().await?;
    lock_active_binding(&mut tx, workspace_binding_id).await?;
    lock_upstream(&mut tx, workspace_binding_id, upstream_id).await?;
    let upstream = sqlx::query_as::<_, WorkspaceGitUpstreamResponse>(include_str!(
        "../sql/workspace_git_upstreams/set_enabled.sql"
    ))
    .bind(workspace_binding_id)
    .bind(upstream_id)
    .bind(enabled)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(upstream)
}

async fn ensure_binding_exists(
    pool: &PgPool,
    workspace_binding_id: i64,
) -> Result<(), GitUpstreamError> {
    let exists = sqlx::query_scalar::<_, bool>(include_str!(
        "../sql/workspace_git_upstreams/binding_exists.sql"
    ))
    .bind(workspace_binding_id)
    .fetch_one(pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(GitUpstreamError::BindingNotFound)
    }
}

async fn lock_active_binding(
    tx: &mut Transaction<'_, Postgres>,
    workspace_binding_id: i64,
) -> Result<(), GitUpstreamError> {
    let binding = sqlx::query_as::<_, (String, bool)>(include_str!(
        "../sql/workspace_git_upstreams/lock_binding.sql"
    ))
    .bind(workspace_binding_id)
    .fetch_optional(&mut **tx)
    .await?;
    match binding {
        None => Err(GitUpstreamError::BindingNotFound),
        Some((lifecycle_state, is_active)) if lifecycle_state == "active" && is_active => Ok(()),
        Some(_) => Err(GitUpstreamError::BindingInactive),
    }
}

async fn lock_authority_binding(
    tx: &mut Transaction<'_, Postgres>,
    workspace_binding_id: i64,
    authority: &MutationAuthority<'_>,
) -> Result<(), GitUpstreamError> {
    let binding = sqlx::query_as::<_, WorkspaceBindingResponse>(include_str!(
        "../sql/lock_workspace_binding_for_write.sql"
    ))
    .bind(workspace_binding_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(GitUpstreamError::BindingNotFound)?;
    validate_authority(&binding, authority)
}

fn validate_authority(
    binding: &WorkspaceBindingResponse,
    authority: &MutationAuthority<'_>,
) -> Result<(), GitUpstreamError> {
    if let MutationAuthority::ApplicationSubject {
        application_id,
        external_user_id,
        ..
    } = authority
    {
        if binding.application_id != *application_id
            || binding.external_user_id != *external_user_id
        {
            return Err(GitUpstreamError::Forbidden);
        }
    }
    if !binding.is_active || binding.lifecycle_state != "active" || binding.archived_at.is_some() {
        return Err(GitUpstreamError::BindingInactive);
    }
    if let MutationAuthority::ApplicationSubject { lease_owner, .. } = authority {
        check_write_lease(binding, Some(*lease_owner))
            .map_err(|_| GitUpstreamError::LeaseConflict)?;
    }
    Ok(())
}

async fn lock_upstream(
    tx: &mut Transaction<'_, Postgres>,
    workspace_binding_id: i64,
    upstream_id: i64,
) -> Result<(), GitUpstreamError> {
    sqlx::query_scalar::<_, i64>(include_str!(
        "../sql/workspace_git_upstreams/lock_upstream.sql"
    ))
    .bind(workspace_binding_id)
    .bind(upstream_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(GitUpstreamError::NotFound)?;
    Ok(())
}

async fn write_credential(
    tx: &mut Transaction<'_, Postgres>,
    workspace_binding_id: i64,
    upstream_id: i64,
    username: &str,
    ciphertext: &[u8],
) -> Result<(), GitUpstreamError> {
    sqlx::query(include_str!(
        "../sql/workspace_git_upstreams/set_credential.sql"
    ))
    .bind(workspace_binding_id)
    .bind(upstream_id)
    .bind(username)
    .bind(ciphertext)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn read_upstream(
    tx: &mut Transaction<'_, Postgres>,
    workspace_binding_id: i64,
    upstream_id: i64,
) -> Result<WorkspaceGitUpstreamResponse, GitUpstreamError> {
    sqlx::query_as::<_, WorkspaceGitUpstreamResponse>(include_str!(
        "../sql/workspace_git_upstreams/get.sql"
    ))
    .bind(workspace_binding_id)
    .bind(upstream_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(GitUpstreamError::NotFound)
}

/// Gateway resolution row. The credential frame is kept for the proxy path
/// but excluded from `Debug` so it can never reach a log line.
#[derive(sqlx::FromRow)]
pub struct GatewayUpstreamRow {
    pub upstream_id: i64,
    pub workspace_binding_id: i64,
    pub remote_alias: String,
    pub target_url: String,
    pub credential_username: Option<String>,
    pub credential_ciphertext: Option<Vec<u8>>,
    pub enabled: bool,
    pub capability_generation: i64,
    pub application_id: i64,
    pub application_active: bool,
}

impl GatewayUpstreamRow {
    /// True when a username is stored without a credential frame. The gateway
    /// must refuse such a row and ask for resupply instead of forwarding it
    /// anonymously.
    pub fn requires_credential_resupply(&self) -> bool {
        self.credential_username.is_some() && self.credential_ciphertext.is_none()
    }
}

impl std::fmt::Debug for GatewayUpstreamRow {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GatewayUpstreamRow")
            .field("upstream_id", &self.upstream_id)
            .field("workspace_binding_id", &self.workspace_binding_id)
            .field("remote_alias", &self.remote_alias)
            .field("enabled", &self.enabled)
            .field("capability_generation", &self.capability_generation)
            .field("application_id", &self.application_id)
            .field("application_active", &self.application_active)
            .field("has_credential", &self.credential_ciphertext.is_some())
            .finish()
    }
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct GatewayAliasRow {
    pub upstream_id: i64,
    pub remote_alias: String,
    pub capability_generation: i64,
}

pub async fn resolve_gateway_upstream(
    pool: &PgPool,
    workspace_binding_id: i64,
    remote_alias: &str,
) -> anyhow::Result<Option<GatewayUpstreamRow>> {
    sqlx::query_as::<_, GatewayUpstreamRow>(include_str!(
        "../sql/workspace_git_upstreams/gateway_resolve.sql"
    ))
    .bind(workspace_binding_id)
    .bind(remote_alias)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn list_gateway_aliases(
    pool: &PgPool,
    workspace_binding_id: i64,
) -> anyhow::Result<Vec<GatewayAliasRow>> {
    sqlx::query_as::<_, GatewayAliasRow>(include_str!(
        "../sql/workspace_git_upstreams/gateway_aliases.sql"
    ))
    .bind(workspace_binding_id)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

fn bounded_page(limit: Option<i64>, offset: Option<i64>) -> (i64, i64) {
    (
        limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
        offset.unwrap_or(0).max(0),
    )
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(database) if database.code().as_deref() == Some("23505"))
}

#[cfg(test)]
#[path = "workspace_git_upstreams/tests.rs"]
mod tests;
