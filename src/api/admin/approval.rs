use std::time::Instant;

use axum::{Json, extract::State};
use axum_extra::extract::cookie::CookieJar;

use super::{shared::record_admin_audit, users::require_admin};
use crate::{
    AppState,
    api::validation::ValidatedJson,
    db::types::{ApprovalSettingsResponse, ApprovalTestResponse, UpdateApprovalSettingsRequest},
    error::AppError,
};

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
    let key_status = state.approval.global_api_key_status(&settings);
    Ok(Json(settings_response(settings, key_status)))
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
    let existing = crate::db::queries::get_approval_settings(&state.db).await?;
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
    let secret = resolve_secret(
        &state,
        request.api_key.take(),
        request.clear_api_key,
        crate::approval::encrypted_secret_from_database(
            existing.api_key_ciphertext,
            existing.api_key_nonce,
            existing.api_key_key_version,
        )?,
    )?;
    let settings =
        crate::db::queries::update_approval_settings(&state.db, &request, secret.as_ref()).await?;
    let key_status = state.approval.global_api_key_status(&settings);
    record_admin_audit(
        &state,
        &admin,
        "admin.approval_settings.update",
        "approval_settings",
        "1".to_string(),
        serde_json::json!({
            "configured": settings.endpoint.is_some() && settings.model.is_some(),
            "enabled": settings.enabled,
            "api_key_configured": key_status.configured,
            "model": settings.model.clone(),
            "timeout_ms": settings.timeout_ms,
            "max_input_bytes": settings.max_input_bytes,
            "max_concurrent": settings.max_concurrent,
            "max_output_tokens": settings.max_output_tokens,
        }),
    )
    .await?;
    Ok(Json(settings_response(settings, key_status)))
}

#[utoipa::path(
    post,
    path = "/api/admin/approval-settings/test",
    tag = "admin-operations",
    responses((status = 200, body = ApprovalTestResponse))
)]
pub async fn test_approval_settings(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<ApprovalTestResponse>, AppError> {
    let admin = require_admin(&state, &jar).await?;
    let settings = crate::db::queries::get_approval_settings(&state.db).await?;
    let model = settings.model.clone();
    let started = Instant::now();
    let result = match state.approval.test_global(&state).await {
        Ok(decision) => ApprovalTestResponse {
            ok: true,
            stage: "review".to_string(),
            message: "Reviewer responded successfully".to_string(),
            latency_ms: started.elapsed().as_millis() as u64,
            model: model.clone(),
            decision: Some(format!("{:?}", decision.decision).to_lowercase()),
            risk: Some(format!("{:?}", decision.risk).to_lowercase()),
            reason_code: Some(decision.reason_code),
        },
        Err(error) => {
            let mut result = probe_error(started, error);
            result.model = model;
            result
        }
    };
    record_admin_audit(
        &state,
        &admin,
        "admin.approval_settings.test",
        "approval_settings",
        "1".to_string(),
        serde_json::json!({
            "ok": result.ok,
            "stage": result.stage,
            "latency_ms": result.latency_ms,
        }),
    )
    .await?;
    Ok(Json(result))
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    })
}

fn settings_response(
    settings: crate::db::types::ApprovalSettingsRecord,
    key_status: crate::approval::ApiKeyStatus,
) -> ApprovalSettingsResponse {
    let usable_key = key_status.configured
        && (key_status.source != "database" || key_status.secret_storage_ready);
    ApprovalSettingsResponse {
        enabled: settings.enabled,
        configured: settings.enabled
            && settings.endpoint.is_some()
            && settings.model.is_some()
            && usable_key,
        api_key_configured: key_status.configured,
        api_key_source: key_status.source.to_string(),
        secret_storage_ready: key_status.secret_storage_ready,
        endpoint: settings.endpoint,
        model: settings.model,
        timeout_ms: settings.timeout_ms,
        max_input_bytes: settings.max_input_bytes,
        max_concurrent: settings.max_concurrent,
        max_output_tokens: settings.max_output_tokens,
        updated_at: settings.updated_at,
    }
}

fn resolve_secret(
    state: &AppState,
    api_key: Option<String>,
    clear: bool,
    existing: Option<crate::secrets::EncryptedSecret>,
) -> Result<Option<crate::secrets::EncryptedSecret>, AppError> {
    if clear {
        return Ok(None);
    }
    let Some(api_key) = api_key else {
        return Ok(existing);
    };
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err(AppError::bad_request("api_key must not be blank"));
    }
    state
        .approval
        .encrypt_api_key(api_key)
        .map(Some)
        .map_err(|error| AppError::service_unavailable(error.to_string()))
}

pub(crate) fn probe_error(
    started: Instant,
    error: crate::approval::ApprovalTestError,
) -> ApprovalTestResponse {
    let (stage, message) = match error {
        crate::approval::ApprovalTestError::DisabledOrNotConfigured => {
            ("configuration", "Reviewer is disabled or not configured")
        }
        crate::approval::ApprovalTestError::ApiKeyMissing => {
            ("configuration", "Reviewer API key is not configured")
        }
        crate::approval::ApprovalTestError::SecretStorage => {
            ("configuration", "Reviewer secret storage is not configured")
        }
        crate::approval::ApprovalTestError::InvalidConfiguration => {
            ("configuration", "Reviewer configuration is invalid")
        }
        crate::approval::ApprovalTestError::Provider(error) => match error {
            desk_foreman_approval::ApprovalError::InputTooLarge => {
                ("request", "Synthetic reviewer request exceeds its limit")
            }
            desk_foreman_approval::ApprovalError::TimedOut => {
                ("timeout", "Reviewer request timed out")
            }
            desk_foreman_approval::ApprovalError::Unavailable => {
                ("connection", "Reviewer endpoint is unavailable")
            }
            desk_foreman_approval::ApprovalError::InvalidResponse => {
                ("response", "Reviewer returned an invalid response")
            }
        },
        crate::approval::ApprovalTestError::Database(_) => {
            ("configuration", "Reviewer configuration lookup failed")
        }
    };
    ApprovalTestResponse {
        ok: false,
        stage: stage.to_string(),
        message: message.to_string(),
        latency_ms: started.elapsed().as_millis() as u64,
        model: None,
        decision: None,
        risk: None,
        reason_code: None,
    }
}
