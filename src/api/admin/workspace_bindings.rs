use std::fs;

use axum::{
    Json,
    extract::{Path, State},
};
use axum_extra::extract::cookie::CookieJar;

use crate::{
    AppState,
    api::validation::ValidatedQuery,
    db::types::{ListWorkspaceBindingsParams, WorkspaceBindingResponse},
    error::AppError,
    workspace::resolve_workspace_binding_root,
};
use runner_protocol::{CancelSessionRequest, RunnerOwner};
use serde_json::json;

use super::{shared::record_admin_audit, users::require_admin};

#[utoipa::path(
    get,
    path = "/api/admin/workspace-bindings",
    tag = "admin-users",
    params(ListWorkspaceBindingsParams),
    responses(
        (status = 200, body = [WorkspaceBindingResponse]),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse)
    )
)]
pub async fn list_workspace_bindings(
    State(state): State<AppState>,
    jar: CookieJar,
    ValidatedQuery(params): ValidatedQuery<ListWorkspaceBindingsParams>,
) -> Result<Json<Vec<WorkspaceBindingResponse>>, AppError> {
    require_admin(&state, &jar).await?;
    let (items, _) = crate::db::queries::list_workspace_bindings(&state.db, &params)
        .await
        .map_err(|error| AppError::bad_request(error.to_string()))?;
    Ok(Json(items))
}

#[utoipa::path(
    get,
    path = "/api/admin/workspace-bindings/{binding_id}",
    tag = "admin-users",
    params(("binding_id" = i64, Path, description = "Workspace binding identifier")),
    responses(
        (status = 200, body = WorkspaceBindingResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse)
    )
)]
pub async fn get_workspace_binding(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(binding_id): Path<i64>,
) -> Result<Json<WorkspaceBindingResponse>, AppError> {
    require_admin(&state, &jar).await?;
    let Some(binding) =
        crate::db::queries::find_workspace_binding_by_id(&state.db, binding_id).await?
    else {
        return Err(AppError::not_found("workspace binding not found"));
    };
    Ok(Json(binding))
}

#[utoipa::path(
    post,
    path = "/api/admin/workspace-bindings/{binding_id}/archive",
    tag = "admin-users",
    params(("binding_id" = i64, Path, description = "Workspace binding identifier")),
    responses((status = 200, body = WorkspaceBindingResponse))
)]
pub async fn archive_workspace_binding(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(binding_id): Path<i64>,
) -> Result<Json<WorkspaceBindingResponse>, AppError> {
    transition_workspace_binding(&state, &jar, binding_id, "archived").await
}

#[utoipa::path(
    post,
    path = "/api/admin/workspace-bindings/{binding_id}/restore",
    tag = "admin-users",
    params(("binding_id" = i64, Path, description = "Workspace binding identifier")),
    responses((status = 200, body = WorkspaceBindingResponse))
)]
pub async fn restore_workspace_binding(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(binding_id): Path<i64>,
) -> Result<Json<WorkspaceBindingResponse>, AppError> {
    transition_workspace_binding(&state, &jar, binding_id, "active").await
}

#[utoipa::path(
    post,
    path = "/api/admin/workspace-bindings/{binding_id}/reset",
    tag = "admin-users",
    params(("binding_id" = i64, Path, description = "Workspace binding identifier")),
    responses((status = 200, body = WorkspaceBindingResponse))
)]
pub async fn reset_workspace_binding(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(binding_id): Path<i64>,
) -> Result<Json<WorkspaceBindingResponse>, AppError> {
    let admin = require_admin(&state, &jar).await?;
    let Some(binding) =
        crate::db::queries::find_workspace_binding_by_id(&state.db, binding_id).await?
    else {
        return Err(AppError::not_found("workspace binding not found"));
    };
    let _ =
        crate::db::queries::set_workspace_binding_state(&state.db, binding_id, "resetting").await?;
    cancel_binding_sessions(&state, binding_id).await?;
    let root = resolve_workspace_binding_root(&state.config.workspace_root, &binding)
        .map_err(AppError::internal)?;
    clear_directory(&root).map_err(AppError::internal)?;
    let application = crate::db::queries::find_application_by_id(&state.db, binding.application_id)
        .await?
        .ok_or_else(|| AppError::not_found("application not found"))?;
    crate::workspace::initialize_workspace_template(
        &state.config.workspace_root,
        &root,
        application.workspace_template.as_deref(),
    )?;
    let binding = crate::db::queries::set_workspace_binding_state(&state.db, binding_id, "active")
        .await?
        .ok_or_else(|| AppError::not_found("workspace binding not found"))?;
    record_admin_audit(
        &state,
        &admin,
        "workspace.reset",
        "workspace_binding",
        binding_id.to_string(),
        json!({}),
    )
    .await?;
    Ok(Json(binding))
}

async fn transition_workspace_binding(
    state: &AppState,
    jar: &CookieJar,
    binding_id: i64,
    lifecycle_state: &str,
) -> Result<Json<WorkspaceBindingResponse>, AppError> {
    let admin = require_admin(state, jar).await?;
    let Some(binding) =
        crate::db::queries::set_workspace_binding_state(&state.db, binding_id, lifecycle_state)
            .await?
    else {
        return Err(AppError::not_found("workspace binding not found"));
    };
    if lifecycle_state == "archived" {
        cancel_binding_sessions(state, binding_id).await?;
    }
    record_admin_audit(
        state,
        &admin,
        if lifecycle_state == "archived" {
            "workspace.archive"
        } else {
            "workspace.restore"
        },
        "workspace_binding",
        binding_id.to_string(),
        json!({ "state": lifecycle_state }),
    )
    .await?;
    Ok(Json(binding))
}

async fn cancel_binding_sessions(state: &AppState, binding_id: i64) -> Result<(), AppError> {
    for session in state.runner.list_sessions().await? {
        if session.owner
            == (RunnerOwner::WorkspaceBinding {
                workspace_binding_id: binding_id,
            })
        {
            let _ = state
                .runner
                .cancel_session(CancelSessionRequest {
                    owner: session.owner,
                    session_key: session.session_key,
                    session_id: session.session_id,
                })
                .await;
        }
    }
    Ok(())
}

fn clear_directory(root: &std::path::Path) -> anyhow::Result<()> {
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}
