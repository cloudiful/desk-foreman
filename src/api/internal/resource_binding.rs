//! Internal resource workspace binding resolution.
//!
//! One bodyless endpoint that resolves the application-scoped active binding
//! of a resource workspace. It delegates to the shared find-or-create
//! resolver, so a replay returns the same binding identity, and it never sets,
//! renews, clears or takes over a write lease: the lease endpoints stay the
//! only legacy lease authority and native admission stays the only native
//! ownership authority.

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use axum_extra::extract::cookie::CookieJar;

use super::shared::resolve_or_create_resource_binding_for_application;

use crate::{AppState, db::types::WorkspaceBindingResponse, error::AppError};

#[utoipa::path(
    post,
    path = "/api/internal/resource-workspaces/{resource_kind}/{resource_id}/binding",
    tag = "internal",
    security(("application_bearer" = [])),
    params(
        ("resource_kind" = String, Path, description = "Resource workspace kind"),
        ("resource_id" = String, Path, description = "Resource workspace identifier")
    ),
    responses(
        (status = 200, body = WorkspaceBindingResponse),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 500, body = crate::error::ErrorResponse)
    )
)]
pub async fn ensure_resource_workspace_binding(
    State(state): State<AppState>,
    _jar: CookieJar,
    headers: HeaderMap,
    Path((resource_kind, resource_id)): Path<(String, String)>,
) -> Result<Json<WorkspaceBindingResponse>, AppError> {
    let (_application_id, binding) = resolve_or_create_resource_binding_for_application(
        &state,
        &headers,
        &resource_kind,
        &resource_id,
    )
    .await?;
    Ok(Json(binding))
}
