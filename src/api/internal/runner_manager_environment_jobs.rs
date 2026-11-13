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
use chrono::Utc;
use runner_protocol::{RunnerJob, RunnerJobResult};
use std::path::Path as FsPath;
use uuid::Uuid;

use crate::{
    AppState,
    db::{
        execution_environment_jobs::{
            DurableJobCompletion, claim_next_assigned_manager_environment_job,
            complete_assigned_manager_environment_job, encode_environment_job_wire_id,
            environment_job_owner, environment_job_wire_kind,
            find_overdue_export_environment_for_manager, is_environment_job_wire_id,
            parse_environment_job_wire_id,
        },
        types::WorkspaceBindingResponse,
    },
    error::AppError,
};

use runner_protocol::execution_environment::ExecutionEnvironmentWorkspaceScope;

/// Derive the server-side dispatch scope for one stored ensure environment.
///
/// This is freshness of a previously authorized intent, not new caller proof:
/// the stored binding must still be active/resource-owned with a live lease,
/// and the relative suffix is derived lexically from the trusted CP root. No
/// stored lease owner is ever treated as supplied caller proof.
fn dispatch_workspace_scope(
    application_id: i64,
    stored_binding_id: Option<i64>,
    binding: Option<&WorkspaceBindingResponse>,
    trusted_root: &str,
) -> Result<Option<ExecutionEnvironmentWorkspaceScope>, &'static str> {
    let Some(workspace_binding_id) = stored_binding_id else {
        return Ok(None);
    };
    if workspace_binding_id <= 0 {
        return Err("workspace scope binding id is not positive");
    }
    let binding = binding.ok_or("workspace binding is missing")?;
    if binding.workspace_binding_id != workspace_binding_id
        || binding.application_id != application_id
    {
        return Err("workspace binding identity does not match the environment");
    }
    if !binding.is_active || binding.lifecycle_state != "active" || binding.archived_at.is_some() {
        return Err("workspace binding is not active");
    }
    let resource_owned = binding
        .resource_kind
        .as_deref()
        .is_some_and(|kind| !kind.trim().is_empty())
        && binding
            .resource_id
            .as_deref()
            .is_some_and(|id| !id.trim().is_empty());
    if !resource_owned {
        return Err("workspace binding is not resource-owned");
    }
    if binding
        .write_lease_owner
        .as_deref()
        .is_none_or(|owner| owner.trim().is_empty())
    {
        return Err("workspace binding has no live write lease");
    }
    let expires = binding
        .write_lease_expires_at
        .ok_or("workspace binding has no lease expiry")?;
    if expires <= Utc::now() {
        return Err("workspace binding lease is expired");
    }

    let root = FsPath::new(&binding.workspace_root);
    let trusted = FsPath::new(trusted_root);
    if !root.is_absolute() || !trusted.is_absolute() {
        return Err("workspace root is not absolute");
    }
    let root = root.to_str().ok_or("workspace root is not utf8")?;
    let trusted = trusted
        .to_str()
        .ok_or("trusted workspace root is not utf8")?;
    let suffix = FsPath::new(root)
        .strip_prefix(FsPath::new(trusted))
        .map_err(|_| "workspace root is not under the trusted root")?;
    let suffix = suffix
        .to_str()
        .ok_or("workspace scope suffix is not utf8")?;
    if suffix.is_empty() || suffix == "." {
        return Err("workspace scope suffix is not strict");
    }
    let scope = ExecutionEnvironmentWorkspaceScope {
        workspace_binding_id,
        workspace_relative_path: suffix.to_string(),
    };
    scope
        .validate()
        .map_err(|_| "workspace scope suffix is invalid")?;
    Ok(Some(scope))
}

#[cfg(test)]
mod scoped_environment_dispatch_tests {
    use chrono::{Duration, Utc};

    use super::WorkspaceBindingResponse;
    use super::dispatch_workspace_scope;

    const APPLICATION_ID: i64 = 7;
    const BINDING_ID: i64 = 11;

    fn binding() -> WorkspaceBindingResponse {
        let now = Utc::now();
        WorkspaceBindingResponse {
            workspace_binding_id: BINDING_ID,
            application_id: APPLICATION_ID,
            external_user_id: "fixture-user".to_string(),
            workspace_key: "artifact:fixture".to_string(),
            external_user_hash: "fixture-hash".to_string(),
            workspace_root: "/srv/workspaces/issue/scope".to_string(),
            is_active: true,
            last_used_at: now,
            created_at: now,
            updated_at: now,
            lifecycle_state: "active".to_string(),
            archived_at: None,
            resource_kind: Some("artifact".to_string()),
            resource_id: Some("fixture".to_string()),
            write_lease_owner: Some("lease:fixture".to_string()),
            write_lease_acquired_at: Some(now),
            write_lease_expires_at: Some(now + Duration::hours(1)),
        }
    }

    #[test]
    fn scoped_environment_dispatch_accepts_only_a_live_active_scope() {
        let binding = binding();
        let scope = dispatch_workspace_scope(
            APPLICATION_ID,
            Some(BINDING_ID),
            Some(&binding),
            "/srv/workspaces",
        )
        .expect("a fresh active binding must dispatch")
        .expect("scoped dispatch has a scope");
        assert_eq!(scope.workspace_binding_id, BINDING_ID);
        assert_eq!(scope.workspace_relative_path, "issue/scope");

        assert!(
            dispatch_workspace_scope(APPLICATION_ID, None, None, "/srv/workspaces")
                .expect("unscoped dispatch")
                .is_none()
        );
    }

    #[test]
    fn scoped_environment_dispatch_refuses_every_invalid_binding() {
        let cases: [(&str, WorkspaceBindingResponse); 8] = [
            ("foreign application", {
                let mut binding = binding();
                binding.application_id += 1;
                binding
            }),
            ("wrong binding id", {
                let mut binding = binding();
                binding.workspace_binding_id += 1;
                binding
            }),
            ("inactive", {
                let mut binding = binding();
                binding.is_active = false;
                binding
            }),
            ("archived", {
                let mut binding = binding();
                binding.archived_at = Some(Utc::now());
                binding
            }),
            ("not resource owned", {
                let mut binding = binding();
                binding.resource_kind = None;
                binding
            }),
            ("no lease owner", {
                let mut binding = binding();
                binding.write_lease_owner = Some("   ".to_string());
                binding
            }),
            ("expired lease", {
                let mut binding = binding();
                binding.write_lease_expires_at = Some(Utc::now() - Duration::seconds(1));
                binding
            }),
            ("outside trusted root", {
                let mut binding = binding();
                binding.workspace_root = "/srv/other/scope".to_string();
                binding
            }),
        ];
        for (label, binding) in cases {
            assert!(
                dispatch_workspace_scope(
                    APPLICATION_ID,
                    Some(BINDING_ID),
                    Some(&binding),
                    "/srv/workspaces",
                )
                .is_err(),
                "{label} must refuse"
            );
        }

        assert!(
            dispatch_workspace_scope(APPLICATION_ID, Some(BINDING_ID), None, "/srv/workspaces")
                .is_err(),
            "a missing binding must refuse"
        );
        assert!(
            dispatch_workspace_scope(APPLICATION_ID, Some(0), Some(&binding()), "/srv/workspaces")
                .is_err(),
            "a nonpositive binding id must refuse"
        );
        let mut root_scope = binding();
        root_scope.workspace_root = "/srv/workspaces".to_string();
        assert!(
            dispatch_workspace_scope(
                APPLICATION_ID,
                Some(BINDING_ID),
                Some(&root_scope),
                "/srv/workspaces"
            )
            .is_err(),
            "an empty suffix must refuse"
        );
        let mut dotdot = binding();
        dotdot.workspace_root = "/srv/workspaces/../escape".to_string();
        assert!(
            dispatch_workspace_scope(
                APPLICATION_ID,
                Some(BINDING_ID),
                Some(&dotdot),
                "/srv/workspaces"
            )
            .is_err(),
            "an escaping suffix must refuse"
        );
    }
}

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
    expire_one_overdue_export_environment(state, manager_id).await?;
    let Some(row) =
        claim_next_assigned_manager_environment_job(&state.db, &state.task_runtime, manager_id)
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
        if row.job_kind == "ensure" {
            // Never degrade a stored workspace-scoped environment into an
            // owner-only job: read the durable row and binding first, derive
            // the server-side relative suffix and refuse before a RunnerJob
            // exists when the stored scope or lease is not dispatchable.
            let environment = crate::db::queries::find_execution_environment(
                &state.db,
                row.application_id,
                row.environment_id,
            )
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| {
                AppError::internal(anyhow::anyhow!(
                    "durable ensure job has no execution environment row"
                ))
            })?;
            let binding = match environment.workspace_binding_id {
                Some(workspace_binding_id) => Some(
                    crate::db::queries::find_workspace_binding_by_id(
                        &state.db,
                        workspace_binding_id,
                    )
                    .await
                    .map_err(AppError::internal)?
                    .ok_or_else(|| {
                        AppError::conflict(
                            "execution environment job workspace scope is not dispatchable",
                        )
                    })?,
                ),
                None => None,
            };
            let trusted_root = state.config.workspace_root.to_str().ok_or_else(|| {
                AppError::conflict("execution environment job workspace scope is not dispatchable")
            })?;
            let scope = dispatch_workspace_scope(
                environment.application_id,
                environment.workspace_binding_id,
                binding.as_ref(),
                trusted_root,
            )
            .map_err(|_| {
                AppError::conflict("execution environment job workspace scope is not dispatchable")
            })?;
            serde_json::to_value(runner_protocol::EnsureExecutionEnvironmentRequest {
                owner: environment_job_owner(&row),
                workspace_scope: scope,
            })
            .map_err(|error| AppError::internal(anyhow::Error::from(error)))?
        } else {
            serde_json::json!({ "owner": environment_job_owner(&row) })
        }
    };
    Ok(Some(RunnerJob {
        job_id: encode_environment_job_wire_id(row.job_id),
        kind: kind.to_string(),
        payload,
    }))
}

/// Expire at most one overdue export environment of the authenticated manager.
///
/// The native expiry of the domain covers the preparation, start and stop
/// jobs; an export job that was never claimed has no other expiry path. The
/// preflight runs before the claim in its own transaction, so the expiry and
/// the later claim never share a transaction. A manager that never polls
/// keeps the existing limitation: nothing expires its export jobs.
async fn expire_one_overdue_export_environment(
    state: &AppState,
    manager_id: i64,
) -> Result<(), AppError> {
    let Some(overdue) = find_overdue_export_environment_for_manager(&state.db, manager_id)
        .await
        .map_err(AppError::internal)?
    else {
        return Ok(());
    };
    crate::db::execution_environment_jobs::expire_execution_environment_jobs(
        &state.db,
        &state.task_runtime,
        overdue.application_id,
        overdue.environment_id,
    )
    .await
    .map_err(AppError::internal)?;
    Ok(())
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
    let outcome = complete_assigned_manager_environment_job(
        &state.db,
        &state.task_runtime,
        manager_id,
        job_id,
        &result,
    )
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
