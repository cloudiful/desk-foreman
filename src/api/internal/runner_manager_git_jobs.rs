//! Git job claim, result, and private credential channel.
//!
//! The manager claims a secret-free job, fetches the transient credential over
//! a separate authenticated endpoint, and reports one closed typed result. No
//! credential or row token enters `RunnerJob.payload`, logs, or the database.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use runner_protocol::{
    GitJobCredential, GitJobErrorCode, GitJobOutcome, GitJobSpec, RunnerJob, RunnerJobResult,
};
use uuid::Uuid;

use crate::{
    AppState,
    db::workspace_git_jobs::{
        lifecycle::{
            CompleteOutcome, claim_next_git_job, complete_git_job, git_job_credential_scope,
        },
        store::find_git_job_by_id,
        types::{GitJobCompletion, GitJobRecord, GitJobResult},
    },
    error::AppError,
    runner::git_job_credentials::GitJobCredentialIdentity,
};

const GIT_JOB_WIRE_PREFIX: &str = "gitjob:";
const GIT_JOB_OPERATION_HEADER: &str = "x-df-git-operation-id";

/// True when a result id uses the reserved Git-job namespace. A malformed
/// prefixed id still routes here and is rejected, never to another handler.
pub(super) fn routes_to_git_handler(job_id: &str) -> bool {
    job_id.starts_with(GIT_JOB_WIRE_PREFIX)
}

fn encode_git_job_wire_id(job_id: Uuid) -> String {
    format!("{GIT_JOB_WIRE_PREFIX}{job_id}")
}

fn parse_git_job_wire_id(job_id: &str) -> Option<Uuid> {
    job_id
        .strip_prefix(GIT_JOB_WIRE_PREFIX)
        .and_then(|value| Uuid::parse_str(value).ok())
}

/// Claim the next pending Git job for this manager and build its secret-free
/// claim spec. A wire-build failure after the claim is recorded as a fixed
/// failure with confirmed cleanup because no helper has started yet.
pub(super) async fn next_assigned_git_job(
    state: &AppState,
    manager_id: i64,
) -> Result<Option<RunnerJob>, AppError> {
    let Some(row) = claim_next_git_job(&state.db, manager_id)
        .await
        .map_err(AppError::internal)?
    else {
        return Ok(None);
    };

    let spec = match build_git_job_spec(state, &row).await {
        Ok(spec) => spec,
        Err(error) => {
            tracing::warn!(
                job_id = %row.job_id,
                error = %error,
                "claimed git job could not build its claim spec"
            );
            fail_claimed_job(state, &row, GitJobErrorCode::Unavailable).await;
            return Ok(None);
        }
    };
    let payload = serde_json::to_value(spec)
        .map_err(|error| AppError::internal(anyhow::Error::from(error)))?;
    Ok(Some(RunnerJob {
        job_id: encode_git_job_wire_id(row.job_id),
        kind: "git_operation".to_string(),
        payload,
    }))
}

/// Build the strict secret-free spec from the claimed row and the
/// authoritative binding row. Any drift is a fixed refusal, never a partial
/// job.
async fn build_git_job_spec(state: &AppState, row: &GitJobRecord) -> anyhow::Result<GitJobSpec> {
    if row.state != "claimed" || row.deadline_at <= Utc::now() {
        anyhow::bail!("git job claim is not live");
    }
    let binding =
        crate::db::queries::find_workspace_binding_by_id(&state.db, row.workspace_binding_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("git job binding is absent"))?;
    let expected_resource = row.code_project_id.to_string();
    if binding.application_id != row.application_id
        || !binding.is_active
        || binding.lifecycle_state != "active"
        || binding.resource_kind.as_deref() != Some("code_project")
        || binding.resource_id.as_deref() != Some(expected_resource.as_str())
        || binding.write_lease_owner.as_deref() != Some(row.lease_owner.as_str())
        || binding.write_lease_acquired_at != Some(row.lease_acquired_at)
        || binding
            .write_lease_expires_at
            .is_none_or(|expires_at| expires_at <= Utc::now())
    {
        anyhow::bail!("git job binding scope changed");
    }
    let owner = runner_protocol::workspace_principal::application_principal_owner(
        row.application_id,
        row.workspace_binding_id,
        &binding.external_user_id,
        &binding.workspace_key,
    )
    .map_err(|error| anyhow::anyhow!(error))?;
    let runner_protocol::RunnerOwner::ApplicationPrincipal {
        principal_digest, ..
    } = &owner
    else {
        anyhow::bail!("git job owner is not an application principal");
    };
    if principal_digest != &row.principal_digest {
        anyhow::bail!("git job principal changed");
    }
    let spec = GitJobSpec {
        job_id: row.job_id,
        operation_id: row.operation_id,
        owner,
        application_id: row.application_id,
        workspace_binding_id: row.workspace_binding_id,
        principal_digest: row.principal_digest.clone(),
        code_project_id: row.code_project_id,
        authorization_generation: row.authorization_generation,
        canonical_branch: row.canonical_branch.clone(),
        expected_base: row.expected_base.clone(),
        reserved_bytes: row.reserved_bytes,
        deadline_unix_ms: row.deadline_at.timestamp_millis(),
        workspace_root: binding.workspace_root,
    };
    spec.validate()
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(spec)
}

async fn fail_claimed_job(state: &AppState, row: &GitJobRecord, code: GitJobErrorCode) {
    let completion = GitJobCompletion::Failed {
        error_summary: code.summary().to_string(),
        cleanup_confirmed: true,
    };
    let _ = complete_git_job(
        &state.db,
        row.job_id,
        &row.token,
        row.assigned_manager_id,
        completion,
    )
    .await;
}

/// Route one Git result to the closed typed completion.
pub(super) async fn complete_git_job_result(
    state: &AppState,
    manager_id: i64,
    result: RunnerJobResult,
) -> Result<StatusCode, AppError> {
    let Some(job_id) = parse_git_job_wire_id(&result.job_id) else {
        return Err(AppError::bad_request("invalid git job id"));
    };
    let Some(payload) = result.result else {
        return Err(AppError::bad_request("git job result is missing"));
    };
    let outcome: GitJobOutcome = serde_json::from_value(payload)
        .map_err(|_| AppError::bad_request("git job result is malformed"))?;
    outcome
        .validate()
        .map_err(|_| AppError::bad_request("git job result is malformed"))?;
    let ok_matches = match &outcome {
        GitJobOutcome::SyncSucceeded { .. } => result.ok,
        GitJobOutcome::Failed { .. } => !result.ok,
    };
    if !ok_matches {
        return Err(AppError::bad_request("git job result is inconsistent"));
    }

    let Some(row) = find_git_job_by_id(&state.db, job_id)
        .await
        .map_err(AppError::internal)?
    else {
        return Err(AppError::not_found("git job not found"));
    };
    if row.assigned_manager_id != manager_id || row.operation_id != outcome.operation_id() {
        return Err(AppError::not_found("git job not found"));
    }

    let completion = completion_for_outcome(&outcome);
    let completed = complete_git_job(&state.db, row.job_id, &row.token, manager_id, completion)
        .await
        .map_err(AppError::internal)?;
    match completed {
        CompleteOutcome::Completed(_) | CompleteOutcome::AlreadyTerminal(_) => {
            state.git_job_credentials.remove(&credential_identity(&row));
            Ok(StatusCode::NO_CONTENT)
        }
        CompleteOutcome::TerminalConflict { .. } => Err(AppError::conflict(
            "git job already recorded a different outcome",
        )),
        CompleteOutcome::NotCompletable { current: None } => {
            Err(AppError::not_found("git job not found"))
        }
        CompleteOutcome::NotCompletable { current: Some(_) } => {
            Err(AppError::conflict("git job is not completable"))
        }
    }
}

fn completion_for_outcome(outcome: &GitJobOutcome) -> GitJobCompletion {
    match outcome {
        GitJobOutcome::SyncSucceeded {
            head,
            base,
            published,
            cleanup_confirmed: true,
            ..
        } => GitJobCompletion::Succeeded {
            result: GitJobResult::SyncSucceeded {
                head: head.clone(),
                base: base.clone(),
                published: *published,
            },
        },
        GitJobOutcome::SyncSucceeded {
            cleanup_confirmed: false,
            ..
        } => GitJobCompletion::Failed {
            error_summary: GitJobErrorCode::ExecutionFailed.summary().to_string(),
            cleanup_confirmed: false,
        },
        GitJobOutcome::Failed {
            error_code,
            cleanup_confirmed,
            ..
        } => GitJobCompletion::Failed {
            error_summary: error_code.summary().to_string(),
            cleanup_confirmed: *cleanup_confirmed,
        },
    }
}

fn credential_identity(row: &GitJobRecord) -> GitJobCredentialIdentity {
    GitJobCredentialIdentity {
        job_id: row.job_id,
        operation_id: row.operation_id,
        assigned_manager_id: row.assigned_manager_id,
        application_id: row.application_id,
        workspace_binding_id: row.workspace_binding_id,
        principal_digest: row.principal_digest.clone(),
        code_project_id: row.code_project_id,
        authorization_generation: row.authorization_generation,
        canonical_branch: row.canonical_branch.clone(),
        deadline_unix_ms: row.deadline_at.timestamp_millis(),
    }
}

#[utoipa::path(
    get,
    path = "/api/internal/runner-managers/{manager_id}/git-jobs/{job_id}/credential",
    tag = "internal",
    params(
        ("manager_id" = i64, Path, description = "Authenticated runner manager identifier"),
        ("job_id" = Uuid, Path, description = "Claimed Git job identifier")
    ),
    responses(
        (status = 200, description = "Transient scoped Git credential with Cache-Control: no-store"),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse)
    )
)]
pub async fn git_job_credential(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((manager_id, job_id)): Path<(i64, Uuid)>,
) -> Result<Response, AppError> {
    let manager =
        crate::api::internal::runner_manager::runner_manager_from_token(&state, &headers).await?;
    if manager.runner_manager_id != manager_id {
        return Err(AppError::not_found("git job not found"));
    }
    let operation_id = headers
        .get(GIT_JOB_OPERATION_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .filter(|value| !value.is_nil())
        .ok_or_else(|| AppError::not_found("git job not found"))?;
    let row = git_job_credential_scope(&state.db, job_id, manager_id)
        .await
        .map_err(AppError::internal)?
        .ok_or_else(|| AppError::not_found("git job not found"))?;
    if row.operation_id != operation_id {
        return Err(AppError::not_found("git job not found"));
    }
    let credential = state
        .git_job_credentials
        .get(&credential_identity(&row))
        .ok_or_else(|| AppError::conflict("credential_unavailable"))?;
    let mut response = Json::<GitJobCredential>(credential).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_prefix_is_reserved_and_roundtrips() {
        let job_id = Uuid::new_v4();
        let encoded = encode_git_job_wire_id(job_id);
        assert!(routes_to_git_handler(&encoded));
        assert_eq!(parse_git_job_wire_id(&encoded), Some(job_id));
        assert!(routes_to_git_handler("gitjob:not-a-uuid"));
        assert_eq!(parse_git_job_wire_id("gitjob:not-a-uuid"), None);
    }

    #[test]
    fn cleanup_false_success_degrades_to_uncertain_failure() {
        let outcome = GitJobOutcome::SyncSucceeded {
            operation_id: Uuid::new_v4(),
            head: "a".repeat(40),
            base: None,
            published: true,
            cleanup_confirmed: false,
        };
        let completion = completion_for_outcome(&outcome);
        assert_eq!(completion.state(), "uncertain");
        assert!(!completion.cleanup_confirmed());
    }
}
