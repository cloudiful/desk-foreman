use crate::{
    AppState,
    db::principal_admission::{self, AdmissionBinding, AdmissionOutcome},
    error::AppError,
};
use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use runner_protocol::{
    PrincipalAdmissionRequest, RunnerOwner, workspace_principal::application_principal_owner,
};

#[cfg(test)]
#[path = "runner_principal_admission_tests.rs"]
mod tests;

fn matches_binding(owner: &RunnerOwner, binding: &AdmissionBinding) -> bool {
    let Some(lease_owner) = binding.write_lease_owner.as_deref() else {
        return false;
    };
    binding.is_active
        && binding.lifecycle_state == "active"
        && binding.archived_at.is_none()
        && binding
            .resource_kind
            .as_ref()
            .is_some_and(|s| !s.is_empty())
        && binding.resource_id.as_ref().is_some_and(|s| !s.is_empty())
        && binding.lease_unexpired == Some(true)
        && application_principal_owner(
            binding.application_id,
            binding.workspace_binding_id,
            lease_owner,
            &binding.workspace_key,
        )
        .is_ok_and(|expected| &expected == owner)
}

#[utoipa::path(
    post, path = "/api/internal/runner-manager/principal-admission", tag = "internal",
    request_body = PrincipalAdmissionRequest,
    security(("runner_manager_bearer" = [])),
    responses((status = 204), (status = 400, body = crate::error::ErrorResponse),
        (status = 401, body = crate::error::ErrorResponse), (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse), (status = 409, body = crate::error::ErrorResponse))
)]
pub async fn principal_admission(
    State(state): State<AppState>,
    headers: HeaderMap,
    payload: Result<Json<PrincipalAdmissionRequest>, JsonRejection>,
) -> Result<StatusCode, AppError> {
    let manager = super::runner_manager::runner_manager_from_token(&state, &headers).await?;
    let Json(request) = payload.map_err(|_| AppError::bad_request("invalid admission request"))?;
    if !request.is_valid() {
        return Err(AppError::bad_request("invalid admission request"));
    }
    let RunnerOwner::ApplicationPrincipal {
        workspace_binding_id,
        ..
    } = &request.owner
    else {
        return Err(AppError::bad_request("invalid principal owner"));
    };
    let binding = principal_admission::find_binding(&state.db, *workspace_binding_id)
        .await?
        .ok_or_else(|| AppError::not_found("workspace binding not found"))?;
    if !matches_binding(&request.owner, &binding) {
        return Err(AppError::conflict(
            "workspace binding identity or lease mismatch",
        ));
    }
    match principal_admission::admit(
        &state.db,
        manager.runner_manager_id,
        &request.owner,
        &request.container_id,
    )
    .await?
    {
        AdmissionOutcome::Admitted(_) => Ok(StatusCode::NO_CONTENT),
        AdmissionOutcome::Fenced => Err(AppError::conflict("principal is fenced")),
    }
}
