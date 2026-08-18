use std::time::Instant;

use axum::{
    Json,
    extract::{Path, State},
};
use axum_extra::extract::cookie::CookieJar;
use serde_json::json;

use crate::{AppState, db::types::ApprovalTestResponse, error::AppError};

use super::{approval::probe_error, shared::record_admin_audit, users::require_admin};

pub(super) fn validate_approval_override(
    mode: Option<&str>,
    endpoint: Option<&str>,
    model: Option<&str>,
    timeout_ms: Option<i64>,
    max_input_bytes: Option<i64>,
    max_concurrent: Option<i64>,
    max_output_tokens: Option<i64>,
    has_api_key: bool,
) -> Result<(), AppError> {
    let mode = mode.unwrap_or("inherit");
    if desk_foreman_approval::ApprovalMode::parse(mode).is_none() {
        return Err(AppError::bad_request(
            "approval_mode must be inherit, disabled, or enabled",
        ));
    }
    if mode == "enabled" {
        let endpoint = endpoint
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| AppError::bad_request("enabled approval requires endpoint"))?;
        if model.is_none_or(|value| value.trim().is_empty()) {
            return Err(AppError::bad_request("enabled approval requires model"));
        }
        if !has_api_key {
            return Err(AppError::bad_request("enabled approval requires API key"));
        }
        crate::approval::validate_endpoint(endpoint)
            .map_err(|error| AppError::bad_request(error.to_string()))?;
        if timeout_ms.is_some_and(|value| !(100..=30_000).contains(&value)) {
            return Err(AppError::bad_request(
                "approval_timeout_ms must be between 100 and 30000",
            ));
        }
        if max_input_bytes.is_some_and(|value| !(1..=524_288).contains(&value)) {
            return Err(AppError::bad_request(
                "approval_max_input_bytes must be between 1 and 524288",
            ));
        }
        if max_concurrent.is_some_and(|value| !(1..=64).contains(&value)) {
            return Err(AppError::bad_request(
                "approval_max_concurrent must be between 1 and 64",
            ));
        }
        if max_output_tokens.is_some_and(|value| !(256..=8_192).contains(&value)) {
            return Err(AppError::bad_request(
                "approval_max_output_tokens must be between 256 and 8192",
            ));
        }
    }
    Ok(())
}

#[utoipa::path(
    post,
    path = "/api/admin/applications/{application_id}/approval-test",
    tag = "admin-users",
    params(("application_id" = i64, Path, description = "Application identifier")),
    responses((status = 200, body = ApprovalTestResponse))
)]
pub async fn test_application_approval(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(application_id): Path<i64>,
) -> Result<Json<ApprovalTestResponse>, AppError> {
    let admin = require_admin(&state, &jar).await?;
    let Some(application) =
        crate::db::queries::find_application_by_id(&state.db, application_id).await?
    else {
        return Err(AppError::not_found("application not found"));
    };
    let started = Instant::now();
    let result = match state.approval.test_application(&state, &application).await {
        Ok(decision) => ApprovalTestResponse {
            ok: true,
            stage: "review".to_string(),
            message: "Reviewer responded successfully".to_string(),
            latency_ms: started.elapsed().as_millis() as u64,
            model: application.approval_model.clone(),
            decision: Some(format!("{:?}", decision.decision).to_lowercase()),
            risk: Some(format!("{:?}", decision.risk).to_lowercase()),
            reason_code: Some(decision.reason_code),
        },
        Err(error) => probe_error(started, error),
    };
    record_admin_audit(
        &state,
        &admin,
        "admin.application_approval.test",
        "application",
        application_id.to_string(),
        json!({
            "ok": result.ok,
            "stage": result.stage,
            "latency_ms": result.latency_ms,
        }),
    )
    .await?;
    Ok(Json(result))
}

pub(super) fn resolve_application_secret(
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
        return Err(AppError::bad_request("approval_api_key must not be blank"));
    }
    state
        .approval
        .encrypt_api_key(api_key)
        .map(Some)
        .map_err(|error| AppError::service_unavailable(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::validate_approval_override;

    #[test]
    fn inherit_does_not_validate_application_only_limits() {
        validate_approval_override(
            Some("inherit"),
            None,
            None,
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            false,
        )
        .expect("inherit should use global limits");
    }

    #[test]
    fn enabled_requires_its_own_key_and_limits() {
        assert!(
            validate_approval_override(
                Some("enabled"),
                Some("https://reviewer.example/v1"),
                Some("reviewer"),
                Some(10_000),
                Some(131_072),
                Some(8),
                Some(1024),
                false,
            )
            .is_err()
        );
        assert!(
            validate_approval_override(
                Some("enabled"),
                Some("https://reviewer.example/v1"),
                Some("reviewer"),
                Some(0),
                Some(131_072),
                Some(8),
                Some(1024),
                true,
            )
            .is_err()
        );
    }
}
