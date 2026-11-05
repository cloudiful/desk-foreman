//! Locked, capacity-bounded admission for durable Git jobs.
//!
//! The admission transaction locks the real binding row first, derives the
//! principal from that row, checks the write lease and native-task guard, then
//! takes one global advisory admission lock and verifies all capacity scopes
//! before inserting. No long-running operation runs inside the transaction.

use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use runner_protocol::workspace_principal::application_principal_owner;
use runner_protocol::{RunnerOwner, is_canonical_branch};

use crate::db::workspace_bindings::native_task_guard::native_task_holds_binding;

use super::types::{
    AdmissionError, AdmissionLimits, GitJobRecord, NewGitJob, valid_expected_base,
    valid_lease_owner, valid_token,
};

#[derive(Debug, FromRow)]
struct LockedGitBinding {
    workspace_binding_id: i64,
    application_id: i64,
    external_user_id: String,
    workspace_key: String,
    is_active: bool,
    lifecycle_state: String,
    resource_kind: Option<String>,
    resource_id: Option<String>,
    write_lease_owner: Option<String>,
    write_lease_acquired_at: Option<DateTime<Utc>>,
    write_lease_expires_at: Option<DateTime<Utc>>,
    db_now: Option<DateTime<Utc>>,
}

#[derive(Debug, FromRow)]
struct GitJobUsage {
    global_count: i64,
    global_bytes: i64,
    binding_count: i64,
    binding_bytes: i64,
    manager_count: i64,
    manager_bytes: i64,
}

pub async fn admit_and_insert_git_job(
    pool: &PgPool,
    job: NewGitJob,
    limits: AdmissionLimits,
) -> Result<GitJobRecord, AdmissionError> {
    validate_job(&job)?;
    let limits = limits.validate()?;
    let mut tx = pool.begin().await?;

    let binding: Option<LockedGitBinding> = sqlx::query_file_as!(
        LockedGitBinding,
        "src/sql/workspace_git_jobs/lock_git_job_binding.sql",
        job.workspace_binding_id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(binding) = binding else {
        return Err(AdmissionError::BindingUnavailable);
    };
    if !binding.is_active
        || binding.lifecycle_state != "active"
        || binding.application_id != job.application_id
    {
        return Err(AdmissionError::BindingUnavailable);
    }
    let expected_project = job.code_project_id.to_string();
    if binding.resource_kind.as_deref() != Some("code_project")
        || binding.resource_id.as_deref() != Some(expected_project.as_str())
    {
        return Err(AdmissionError::ResourceMismatch);
    }
    if binding.write_lease_owner.as_deref() != Some(job.lease_owner.as_str())
        || binding.write_lease_acquired_at != Some(job.lease_acquired_at)
    {
        return Err(AdmissionError::LeaseMismatch);
    }
    let Some(lease_expires_at) = binding.write_lease_expires_at else {
        return Err(AdmissionError::LeaseExpired);
    };
    let db_now = binding.db_now.ok_or(AdmissionError::BindingUnavailable)?;
    if lease_expires_at <= db_now {
        return Err(AdmissionError::LeaseExpired);
    }
    if job.deadline_at <= db_now || job.deadline_at > lease_expires_at {
        return Err(AdmissionError::DeadlineInvalid);
    }
    if native_task_holds_binding(&mut tx, binding.workspace_binding_id)
        .await?
        .is_some()
    {
        return Err(AdmissionError::NativeTaskHolds);
    }
    let owner = application_principal_owner(
        binding.application_id,
        binding.workspace_binding_id,
        &binding.external_user_id,
        &binding.workspace_key,
    )
    .map_err(|_| AdmissionError::InvalidInput)?;
    let RunnerOwner::ApplicationPrincipal {
        principal_digest, ..
    } = owner
    else {
        return Err(AdmissionError::InvalidInput);
    };

    sqlx::query(include_str!(
        "../../sql/workspace_git_jobs/lock_git_job_admission.sql"
    ))
    .execute(&mut *tx)
    .await?;

    let usage: GitJobUsage = sqlx::query_as(include_str!(
        "../../sql/workspace_git_jobs/active_git_job_usage.sql"
    ))
    .bind(job.workspace_binding_id)
    .bind(job.assigned_manager_id)
    .fetch_one(&mut *tx)
    .await?;

    if usage.binding_count > 0 {
        return Err(AdmissionError::BindingBusy);
    }
    let global_count = add(usage.global_count, 1)?;
    let binding_count = add(usage.binding_count, 1)?;
    let manager_count = add(usage.manager_count, 1)?;
    let global_bytes = add(usage.global_bytes, job.reserved_bytes)?;
    let binding_bytes = add(usage.binding_bytes, job.reserved_bytes)?;
    let manager_bytes = add(usage.manager_bytes, job.reserved_bytes)?;
    if global_count > limits.max_active
        || binding_count > limits.max_per_binding
        || manager_count > limits.max_per_manager
        || global_bytes > limits.max_bytes
        || binding_bytes > limits.max_bytes
        || manager_bytes > limits.max_bytes_per_manager
    {
        return Err(AdmissionError::CapacityExceeded);
    }

    let inserted: Option<GitJobRecord> = sqlx::query_file_as!(
        GitJobRecord,
        "src/sql/workspace_git_jobs/insert_git_job.sql",
        job.job_id,
        &job.token,
        job.operation_id,
        job.application_id,
        job.workspace_binding_id,
        job.assigned_manager_id,
        job.code_project_id,
        job.authorization_generation,
        &job.canonical_branch,
        job.expected_base.as_deref(),
        &principal_digest,
        &job.lease_owner,
        job.lease_acquired_at,
        job.reserved_bytes,
        job.deadline_at,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(inserted) = inserted else {
        return Err(AdmissionError::BindingBusy);
    };
    tx.commit().await?;
    Ok(inserted)
}

pub async fn find_git_job_by_id(
    pool: &PgPool,
    job_id: Uuid,
) -> anyhow::Result<Option<GitJobRecord>> {
    sqlx::query_file_as!(
        GitJobRecord,
        "src/sql/workspace_git_jobs/find_git_job_by_id.sql",
        job_id,
    )
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

fn validate_job(job: &NewGitJob) -> Result<(), AdmissionError> {
    if job.job_id.is_nil()
        || job.operation_id.is_nil()
        || job.code_project_id.is_nil()
        || job.authorization_generation.is_nil()
        || job.application_id <= 0
        || job.workspace_binding_id <= 0
        || job.assigned_manager_id <= 0
        || !valid_token(&job.token)
        || !is_canonical_branch(&job.canonical_branch)
        || !valid_expected_base(job.expected_base.as_deref())
        || !valid_lease_owner(&job.lease_owner)
        || job.reserved_bytes <= 0
    {
        return Err(AdmissionError::InvalidInput);
    }
    Ok(())
}

fn add(left: i64, right: i64) -> Result<i64, AdmissionError> {
    left.checked_add(right)
        .ok_or(AdmissionError::CapacityExceeded)
}

/// The single active job that owns this binding, if one exists.
pub async fn find_active_git_job_for_binding(
    pool: &PgPool,
    workspace_binding_id: i64,
    application_id: i64,
) -> anyhow::Result<Option<GitJobRecord>> {
    if workspace_binding_id <= 0 || application_id <= 0 {
        anyhow::bail!("invalid active git job lookup");
    }
    sqlx::query_file_as!(
        GitJobRecord,
        "src/sql/workspace_git_jobs/find_active_git_job_for_binding.sql",
        workspace_binding_id,
        application_id,
    )
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

/// Exact identity and strict success result for proof-only reconciliation.
pub struct GitJobReconcile<'a> {
    pub job_id: Uuid,
    pub token: &'a str,
    pub assigned_manager_id: i64,
    pub operation_id: Uuid,
    pub application_id: i64,
    pub workspace_binding_id: i64,
    pub expected_base: Option<&'a str>,
    pub canonical_branch: &'a str,
    pub principal_digest: &'a str,
    pub head: &'a str,
}

/// Compare-and-set one uncertain job to succeeded from externally confirmed
/// publication proof. Never constructs caller-provided result JSON.
pub async fn reconcile_uncertain_git_job(
    pool: &PgPool,
    reconcile: &GitJobReconcile<'_>,
) -> anyhow::Result<Option<GitJobRecord>> {
    if !valid_token(reconcile.token)
        || reconcile.job_id.is_nil()
        || reconcile.operation_id.is_nil()
        || reconcile.assigned_manager_id <= 0
        || reconcile.application_id <= 0
        || reconcile.workspace_binding_id <= 0
        || !valid_expected_base(reconcile.expected_base)
        || !is_canonical_branch(reconcile.canonical_branch)
        || !super::types::is_lower_hex_exact(reconcile.principal_digest, 64)
    {
        anyhow::bail!("invalid git job reconciliation input");
    }
    let completion = super::types::GitJobCompletion::Succeeded {
        result: super::types::GitJobResult::SyncSucceeded {
            head: reconcile.head.to_owned(),
            base: reconcile.expected_base.map(str::to_owned),
            published: true,
        },
    };
    let result = completion
        .result_value()?
        .ok_or_else(|| anyhow::anyhow!("invalid git job reconciliation result"))?;
    sqlx::query_file_as!(
        GitJobRecord,
        "src/sql/workspace_git_jobs/cas_reconcile_uncertain_git_job.sql",
        reconcile.job_id,
        reconcile.token,
        reconcile.assigned_manager_id,
        reconcile.operation_id,
        reconcile.application_id,
        reconcile.workspace_binding_id,
        reconcile.expected_base,
        result,
        reconcile.canonical_branch,
        reconcile.principal_digest,
    )
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}
