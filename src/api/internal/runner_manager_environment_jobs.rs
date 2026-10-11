//! Durable assigned-manager environment-job transport (issue 742 P2).
//!
//! Thin wiring between the internal runner-manager routes and the durable
//! environment-job dispatch helpers. The reserved opaque wire identity
//! `envjob:<UUID>` keeps durable jobs disjoint from the legacy in-memory
//! broker, so a result either routes here or to the unchanged legacy path,
//! never both. The authenticated manager identity is established by the
//! caller before either route runs.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use runner_protocol::{RunnerJob, RunnerJobResult};
use uuid::Uuid;

use crate::{
    AppState,
    db::execution_environment_jobs::{
        DurableJobCompletion, claim_next_assigned_manager_environment_job,
        complete_assigned_manager_environment_job, encode_environment_job_wire_id,
        environment_job_owner, environment_job_wire_kind, is_environment_job_wire_id,
        parse_environment_job_wire_id,
    },
    error::AppError,
};

/// True when a result id uses the reserved durable namespace. A malformed
/// prefixed id still routes here and is rejected, never to the legacy broker.
pub(super) fn routes_to_durable_handler(job_id: &str) -> bool {
    is_environment_job_wire_id(job_id)
}

/// Claim the next pending durable job assigned to `manager_id`, if any.
///
/// Legacy ensure/stop payloads remain owner-only. Preparation and task
/// execution use strict shared DTOs built entirely from stored task, seed and
/// environment identity; no caller-selected host input enters the manager job.
pub(super) async fn next_assigned_environment_job(
    state: &AppState,
    manager_id: i64,
) -> Result<Option<RunnerJob>, AppError> {
    let Some(row) = claim_next_assigned_manager_environment_job(&state.db, manager_id)
        .await
        .map_err(AppError::internal)?
    else {
        return Ok(None);
    };
    let kind = environment_job_wire_kind(&row).map_err(AppError::internal)?;
    // A preparation carries the seed's identity so the manager knows exactly
    // what to fetch and verify; the bytes themselves come from the dedicated
    // authenticated seed route. Legacy ensure/stop keep the owner-only payload.
    let payload = if row.job_kind == "prepare_native_task" {
        let task_id = row.native_task_id.ok_or_else(|| {
            AppError::internal(anyhow::anyhow!("preparation job carries no native task"))
        })?;
        let seed = crate::db::execution_environments::tasks::seeds::preparation_seed_identity(
            &state.db,
            row.application_id,
            row.environment_id,
            task_id,
        )
        .await
        .map_err(|error| AppError::internal(anyhow::anyhow!(error.code())))?
        .ok_or_else(|| {
            AppError::internal(anyhow::anyhow!("preparation task has no sealed seed"))
        })?;
        serde_json::to_value(runner_protocol::PrepareNativeTaskRequest {
            owner: environment_job_owner(&row),
            task_id,
            workspace_binding_id: seed.workspace_binding_id,
            manifest_sha256: seed.manifest_sha256,
            file_count: seed.file_count,
            total_bytes: seed.total_bytes,
        })
        .map_err(|error| AppError::internal(anyhow::Error::from(error)))?
    } else if row.job_kind == "start_native_task"
        || (row.job_kind == "stop" && row.native_task_id.is_some())
    {
        crate::db::execution_environments::tasks::native_execution::request_for_native_job(
            &state.db, &row,
        )
        .await
        .map_err(AppError::internal)?
    } else if row.job_kind == "export_native_task" {
        let request = crate::db::execution_environments::tasks::exports::request_for_export_job(
            &state.db, &row,
        )
        .await
        .map_err(AppError::internal)?;
        serde_json::to_value(request)
            .map_err(|error| AppError::internal(anyhow::Error::from(error)))?
    } else {
        serde_json::json!({ "owner": environment_job_owner(&row) })
    };
    Ok(Some(RunnerJob {
        job_id: encode_environment_job_wire_id(row.job_id),
        kind: kind.to_string(),
        payload,
    }))
}

/// Route one durable result id to the durable handler and map its typed
/// outcome to an HTTP status.
pub(super) async fn complete_environment_job_result(
    state: &AppState,
    manager_id: i64,
    result: RunnerJobResult,
) -> Result<StatusCode, AppError> {
    let Some(job_id) = parse_environment_job_wire_id(&result.job_id) else {
        return Err(AppError::bad_request(
            "invalid execution environment job id",
        ));
    };
    let outcome = complete_assigned_manager_environment_job(&state.db, manager_id, job_id, &result)
        .await
        .map_err(AppError::internal)?;
    match outcome {
        DurableJobCompletion::Completed | DurableJobCompletion::AlreadyTerminal => {
            Ok(StatusCode::NO_CONTENT)
        }
        DurableJobCompletion::NotFound => {
            Err(AppError::not_found("execution environment job not found"))
        }
        DurableJobCompletion::ForeignManager => Err(AppError::forbidden(
            "execution environment job belongs to another runner manager",
        )),
        DurableJobCompletion::Inconsistent(reason) => Err(AppError::bad_request(reason)),
        DurableJobCompletion::TerminalConflict => Err(AppError::conflict(
            "execution environment job already recorded a different outcome",
        )),
        DurableJobCompletion::NotCompletable => Err(AppError::conflict(
            "execution environment job is not claimable by this manager",
        )),
    }
}

#[utoipa::path(
    get,
    path = "/api/internal/runner-managers/{manager_id}/environment-jobs/{job_id}/task-seed",
    tag = "internal",
    params(
        ("manager_id" = i64, Path, description = "Authenticated runner manager identifier"),
        ("job_id" = Uuid, Path, description = "Durable preparation job identifier")
    ),
    responses(
        (status = 200, body = crate::api::internal::execution_environments::tasks::seeds::types::NativeTaskSeedContent, description = "The sealed seed with its contents, in canonical path order"),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse, description = "Foreign, absent or non-retrievable preparation job")
    )
)]
pub async fn task_seed_for_preparation_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((manager_id, job_id)): Path<(i64, Uuid)>,
) -> Result<
    Json<crate::api::internal::execution_environments::tasks::seeds::types::NativeTaskSeedContent>,
    AppError,
> {
    let manager =
        crate::api::internal::runner_manager::runner_manager_from_token(&state, &headers).await?;
    // The path identity must be the authenticated identity.
    if manager.runner_manager_id != manager_id {
        return Err(AppError::not_found("preparation job not found"));
    }
    let content = crate::db::execution_environments::tasks::preparation_seed_for_manager(
        &state.db, manager_id, job_id,
    )
    .await
    .map_err(|error| match error {
        // A foreign, absent or non-live job discloses nothing: it is a plain
        // 404, never a 500 that would hint at internals.
        crate::db::execution_environments::tasks::seeds::SeedError::NotFound => {
            AppError::not_found("preparation job not found")
        }
        other => AppError::internal(anyhow::anyhow!(other.code())),
    })?;
    Ok(Json(content.into()))
}
