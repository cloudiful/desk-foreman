use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use axum_extra::extract::cookie::CookieJar;
use serde_json::json;

use crate::{
    AppState,
    api::validation::ValidatedJson,
    db::types::{
        ApplicationResponse, ApplicationTokenResponse, CreateApplicationRequest,
        CreateApplicationTokenRequest, CreateApplicationTokenResponse, UpdateApplicationRequest,
        UpdateApplicationTokenRequest,
    },
    error::AppError,
};

use super::{
    shared::{map_db_conflict, record_admin_audit},
    users::require_admin,
};

fn validate_approval_override(
    mode: Option<&str>,
    endpoint: Option<&str>,
    model: Option<&str>,
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
        crate::approval::validate_endpoint(endpoint)
            .map_err(|error| AppError::bad_request(error.to_string()))?;
    }
    Ok(())
}

#[utoipa::path(
    get,
    path = "/api/admin/applications",
    tag = "admin-users",
    responses(
        (status = 200, body = [ApplicationResponse]),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse)
    )
)]
pub async fn list_applications(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<Vec<ApplicationResponse>>, AppError> {
    require_admin(&state, &jar).await?;
    Ok(Json(
        crate::db::queries::list_applications(&state.db).await?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/admin/applications",
    tag = "admin-users",
    request_body = CreateApplicationRequest,
    responses(
        (status = 201, body = ApplicationResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse)
    )
)]
pub async fn create_application(
    State(state): State<AppState>,
    jar: CookieJar,
    ValidatedJson(mut request): ValidatedJson<CreateApplicationRequest>,
) -> Result<(StatusCode, Json<ApplicationResponse>), AppError> {
    let admin = require_admin(&state, &jar).await?;
    normalize_approval_override(
        &mut request.approval_mode,
        &mut request.approval_endpoint,
        &mut request.approval_model,
    );
    validate_approval_override(
        request.approval_mode.as_deref(),
        request.approval_endpoint.as_deref(),
        request.approval_model.as_deref(),
    )?;
    let application = crate::db::queries::create_application(&state.db, &request)
        .await
        .map_err(map_db_conflict)?;
    record_admin_audit(
        &state,
        &admin,
        "admin.application.create",
        "application",
        application.application_id.to_string(),
        json!({ "name": application.name }),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(application)))
}

#[utoipa::path(
    patch,
    path = "/api/admin/applications/{application_id}",
    tag = "admin-users",
    request_body = UpdateApplicationRequest,
    params(("application_id" = i64, Path, description = "Application identifier")),
    responses(
        (status = 200, body = ApplicationResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
        (status = 409, body = crate::error::ErrorResponse)
    )
)]
pub async fn update_application(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(application_id): Path<i64>,
    ValidatedJson(mut request): ValidatedJson<UpdateApplicationRequest>,
) -> Result<Json<ApplicationResponse>, AppError> {
    let admin = require_admin(&state, &jar).await?;
    normalize_approval_override(
        &mut request.approval_mode,
        &mut request.approval_endpoint,
        &mut request.approval_model,
    );
    validate_approval_override(
        request.approval_mode.as_deref(),
        request.approval_endpoint.as_deref(),
        request.approval_model.as_deref(),
    )?;
    let Some(application) =
        crate::db::queries::update_application(&state.db, application_id, &request)
            .await
            .map_err(map_db_conflict)?
    else {
        return Err(AppError::not_found("application not found"));
    };
    record_admin_audit(
        &state,
        &admin,
        "admin.application.update",
        "application",
        application.application_id.to_string(),
        json!({ "name": application.name, "is_active": application.is_active }),
    )
    .await?;
    Ok(Json(application))
}

fn normalize_approval_override(
    mode: &mut Option<String>,
    endpoint: &mut Option<String>,
    model: &mut Option<String>,
) {
    for value in [mode, endpoint, model] {
        *value = value.take().and_then(|value| {
            let value = value.trim().to_string();
            (!value.is_empty()).then_some(value)
        });
    }
}

#[utoipa::path(
    get,
    path = "/api/admin/application-tokens",
    tag = "admin-users",
    responses(
        (status = 200, body = [ApplicationTokenResponse]),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse)
    )
)]
pub async fn list_application_tokens(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<Vec<ApplicationTokenResponse>>, AppError> {
    require_admin(&state, &jar).await?;
    Ok(Json(
        crate::db::queries::list_application_tokens(&state.db).await?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/admin/application-tokens",
    tag = "admin-users",
    request_body = CreateApplicationTokenRequest,
    responses(
        (status = 201, body = CreateApplicationTokenResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse)
    )
)]
pub async fn create_application_token(
    State(state): State<AppState>,
    jar: CookieJar,
    ValidatedJson(request): ValidatedJson<CreateApplicationTokenRequest>,
) -> Result<(StatusCode, Json<CreateApplicationTokenResponse>), AppError> {
    let admin = require_admin(&state, &jar).await?;
    let Some(_) =
        crate::db::queries::find_application_by_id(&state.db, request.application_id).await?
    else {
        return Err(AppError::not_found("application not found"));
    };
    let (token, metadata) =
        crate::db::queries::create_application_token(&state.db, &request).await?;
    record_admin_audit(
        &state,
        &admin,
        "admin.application_token.create",
        "application_token",
        metadata.token_id.to_string(),
        json!({ "application_id": metadata.application_id, "token_name": metadata.token_name }),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(CreateApplicationTokenResponse { token, metadata }),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/admin/application-tokens/{token_id}",
    tag = "admin-users",
    params(("token_id" = i64, Path, description = "Application token identifier")),
    responses(
        (status = 204),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse)
    )
)]
pub async fn delete_application_token(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(token_id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let admin = require_admin(&state, &jar).await?;
    let Some(token) = crate::db::queries::find_application_token_by_id(&state.db, token_id).await?
    else {
        return Err(AppError::not_found("application token not found"));
    };
    if !crate::db::queries::revoke_application_token(&state.db, token_id).await? {
        return Err(AppError::not_found("application token not found"));
    }
    record_admin_audit(
        &state,
        &admin,
        "admin.application_token.revoke",
        "application_token",
        token_id.to_string(),
        json!({ "application_id": token.application_id, "token_name": token.token_name }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    patch,
    path = "/api/admin/application-tokens/{token_id}",
    tag = "admin-users",
    params(("token_id" = i64, Path, description = "Application token identifier")),
    request_body = UpdateApplicationTokenRequest,
    responses((status = 200, body = ApplicationTokenResponse))
)]
pub async fn update_application_token(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(token_id): Path<i64>,
    ValidatedJson(request): ValidatedJson<UpdateApplicationTokenRequest>,
) -> Result<Json<ApplicationTokenResponse>, AppError> {
    let admin = require_admin(&state, &jar).await?;
    let Some(token) =
        crate::db::queries::update_application_token(&state.db, token_id, &request).await?
    else {
        return Err(AppError::not_found("application token not found"));
    };
    record_admin_audit(&state, &admin, "admin.application_token.update", "application_token", token_id.to_string(), json!({
        "application_id": token.application_id, "scopes": token.scopes, "expires_at": token.expires_at
    })).await?;
    Ok(Json(token))
}
