use axum::{Json, extract::State};
use axum_extra::extract::cookie::CookieJar;
use runner_protocol::{RunnerOwner, RunnerSessionStatus};

use crate::{
    AppState,
    api::validation::ValidatedQuery,
    db::types::{
        AuditLogResponse, ListAuditLogsParams, ListRunnerSessionsParams, ListWorkspaceRunnersParams,
        OperationsSummary, Page, RunnerSessionResponse, WorkspaceRunnerResponse,
    },
    error::AppError,
};

use super::users::require_admin;

#[utoipa::path(
    get,
    path = "/api/admin/audit-logs",
    tag = "admin-operations",
    params(ListAuditLogsParams),
    responses((status = 200, body = Page<AuditLogResponse>))
)]
pub async fn list_audit_logs(
    State(state): State<AppState>,
    jar: CookieJar,
    ValidatedQuery(params): ValidatedQuery<ListAuditLogsParams>,
) -> Result<Json<Page<AuditLogResponse>>, AppError> {
    require_admin(&state, &jar).await?;
    Ok(Json(
        crate::db::queries::list_audit_logs(&state.db, &params).await?,
    ))
}

#[utoipa::path(
    get,
    path = "/api/admin/workspace-runners",
    tag = "admin-operations",
    params(ListWorkspaceRunnersParams),
    responses((status = 200, body = Page<WorkspaceRunnerResponse>))
)]
pub async fn list_workspace_runners(
    State(state): State<AppState>,
    jar: CookieJar,
    ValidatedQuery(params): ValidatedQuery<ListWorkspaceRunnersParams>,
) -> Result<Json<Page<WorkspaceRunnerResponse>>, AppError> {
    require_admin(&state, &jar).await?;
    let page = crate::db::queries::list_workspace_runners(&state.db, &params).await?;
    Ok(Json(Page {
        items: page
            .items
            .into_iter()
            .map(workspace_runner_response)
            .collect(),
        total: page.total,
        limit: page.limit,
        offset: page.offset,
    }))
}

#[utoipa::path(
    get,
    path = "/api/admin/runner-sessions",
    tag = "admin-operations",
    params(ListRunnerSessionsParams),
    responses((status = 200, body = Page<RunnerSessionResponse>))
)]
pub async fn list_runner_sessions(
    State(state): State<AppState>,
    jar: CookieJar,
    ValidatedQuery(params): ValidatedQuery<ListRunnerSessionsParams>,
) -> Result<Json<Page<RunnerSessionResponse>>, AppError> {
    require_admin(&state, &jar).await?;
    let limit = params.limit.unwrap_or(100);
    let offset = params.offset.unwrap_or(0);
    let all = state.runner.list_sessions().await?;
    let filtered: Vec<RunnerSessionStatus> = all
        .into_iter()
        .filter(|session| match &params.owner_kind {
            Some(kind) => matches!(
                (&session.owner, kind.as_str()),
                (RunnerOwner::InternalUser { .. }, "user")
                    | (RunnerOwner::WorkspaceBinding { .. }, "workspace_binding")
            ),
            None => true,
        })
        .filter(|session| match &params.state {
            Some(state_filter) => session.state == *state_filter,
            None => true,
        })
        .collect();
    let total = filtered.len() as i64;
    let items: Vec<RunnerSessionStatus> = filtered
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();
    Ok(Json(Page {
        items: items.into_iter().map(session_response).collect(),
        total,
        limit,
        offset,
    }))
}

#[utoipa::path(
    get,
    path = "/api/admin/operations/summary",
    tag = "admin-operations",
    responses((status = 200, body = OperationsSummary))
)]
pub async fn operations_summary(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<OperationsSummary>, AppError> {
    require_admin(&state, &jar).await?;
    let (
        active_runners,
        failed_operations,
        archived_workspaces,
        runner_managers_total,
        runner_managers_online,
        runner_managers_offline,
        runner_managers_disabled,
    ) =
        crate::db::queries::operations_summary(&state.db).await?;
    let active_sessions = state
        .runner
        .list_sessions()
        .await?
        .into_iter()
        .filter(|session| matches!(session.state.as_str(), "running" | "pending"))
        .count() as i64;
    Ok(Json(OperationsSummary {
        active_runners,
        active_sessions,
        failed_operations,
        archived_workspaces,
        runner_managers_total,
        runner_managers_online,
        runner_managers_offline,
        runner_managers_disabled,
    }))
}

fn workspace_runner_response(
    row: crate::db::workspace_runners::WorkspaceRunnerRecord,
) -> WorkspaceRunnerResponse {
    WorkspaceRunnerResponse {
        runner_id: row.runner_id,
        owner_kind: row.owner_kind,
        owner_user_id: row.owner_user_id,
        owner_workspace_binding_id: row.owner_workspace_binding_id,
        container_name: row.container_name,
        container_id: row.container_id,
        runtime: row.runtime,
        runtime_class: row.runtime_class,
        image_name: row.image_name,
        status: row.status,
        network_enabled: row.network_enabled,
        workspace_root: row.workspace_root,
        last_active_at: row.last_active_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
        last_error: row.last_error,
    }
}

fn session_response(row: RunnerSessionStatus) -> RunnerSessionResponse {
    let (owner_kind, owner_id) = match row.owner {
        RunnerOwner::InternalUser { user_id } => ("user".to_string(), user_id),
        RunnerOwner::WorkspaceBinding {
            workspace_binding_id,
        } => ("workspace_binding".to_string(), workspace_binding_id),
    };
    RunnerSessionResponse {
        session_id: row.session_id,
        owner_kind,
        owner_id,
        session_key: row.session_key,
        state: row.state,
        exit_code: row.exit_code,
        timed_out: row.timed_out,
        wall_time_seconds: row.wall_time_seconds,
    }
}
