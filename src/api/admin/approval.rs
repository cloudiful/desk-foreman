use axum::{Json, extract::State};
use axum_extra::extract::cookie::CookieJar;

use crate::{
    AppState,
    api::validation::ValidatedJson,
    db::types::{ApprovalSettingsResponse, UpdateApprovalSettingsRequest},
    error::AppError,
};

use super::{shared::record_admin_audit, users::require_admin};

#[utoipa::path(
    get,
    path = "/api/admin/approval-settings",
    tag = "admin-operations",
    responses((status = 200, body = ApprovalSettingsResponse))
)]
pub async fn get_approval_settings(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<ApprovalSettingsResponse>, AppError> {
    require_admin(&state, &jar).await?;
    let settings = crate::db::queries::get_approval_settings(&state.db).await?;
    Ok(Json(settings_response(settings)))
}

#[utoipa::path(
    patch,
    path = "/api/admin/approval-settings",
    tag = "admin-operations",
    request_body = UpdateApprovalSettingsRequest,
    responses((status = 200, body = ApprovalSettingsResponse))
)]
pub async fn update_approval_settings(
    State(state): State<AppState>,
    jar: CookieJar,
    ValidatedJson(mut request): ValidatedJson<UpdateApprovalSettingsRequest>,
) -> Result<Json<ApprovalSettingsResponse>, AppError> {
    let admin = require_admin(&state, &jar).await?;
    request.endpoint = normalize_optional(request.endpoint);
    request.model = normalize_optional(request.model);
    match (&request.endpoint, &request.model) {
        (Some(endpoint), Some(_)) => crate::approval::validate_endpoint(endpoint)
            .map_err(|error| AppError::bad_request(error.to_string()))?,
        (None, None) => {}
        _ => {
            return Err(AppError::bad_request(
                "endpoint and model must be configured together",
            ));
        }
    }
    let settings = crate::db::queries::update_approval_settings(&state.db, &request).await?;
    record_admin_audit(
        &state,
        &admin,
        "admin.approval_settings.update",
        "approval_settings",
        "1".to_string(),
        serde_json::json!({
            "configured": settings.endpoint.is_some() && settings.model.is_some(),
            "model": settings.model.clone(),
            "timeout_ms": settings.timeout_ms,
            "max_input_bytes": settings.max_input_bytes,
            "max_concurrent": settings.max_concurrent,
        }),
    )
    .await?;
    Ok(Json(settings_response(settings)))
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    })
}

fn settings_response(
    settings: crate::db::types::ApprovalSettingsRecord,
) -> ApprovalSettingsResponse {
    ApprovalSettingsResponse {
        configured: settings.endpoint.is_some() && settings.model.is_some(),
        api_key_configured: std::env::var("APPROVAL_API_KEY")
            .ok()
            .or_else(|| std::env::var("OPENAI_API_KEY").ok())
            .is_some_and(|value| !value.is_empty()),
        endpoint: settings.endpoint,
        model: settings.model,
        timeout_ms: settings.timeout_ms,
        max_input_bytes: settings.max_input_bytes,
        max_concurrent: settings.max_concurrent,
        updated_at: settings.updated_at,
    }
}
