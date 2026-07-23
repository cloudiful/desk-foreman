use axum::{Json, extract::State, http::HeaderMap, http::StatusCode};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::{Duration as ChronoDuration, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::{
    AppState,
    actor::actor_from_web_user,
    api::validation::ValidatedJson,
    auth,
    db::{
        audit::AuditLogEntry,
        queries,
        types::{AuthLoginRequest, AuthMeResponse},
    },
    error::AppError,
};

fn build_session_cookie(state: &AppState, value: &str, max_age_seconds: i64) -> Cookie<'static> {
    Cookie::build((state.config.web_cookie_name.clone(), value.to_string()))
        .http_only(true)
        .path("/")
        .same_site(SameSite::Lax)
        .secure(state.config.web_cookie_secure)
        .max_age(time::Duration::seconds(max_age_seconds))
        .build()
}

pub(crate) async fn current_user_from_jar(
    state: &AppState,
    jar: &CookieJar,
) -> Result<Option<(Uuid, crate::actor::ActorContext)>, AppError> {
    current_user_from_jar_with_touch(state, jar, true).await
}

async fn current_user_from_jar_with_touch(
    state: &AppState,
    jar: &CookieJar,
    touch: bool,
) -> Result<Option<(Uuid, crate::actor::ActorContext)>, AppError> {
    let Some(cookie) = jar.get(&state.config.web_cookie_name) else {
        return Ok(None);
    };
    let session_id =
        Uuid::parse_str(cookie.value()).map_err(|_| AppError::unauthorized("invalid session"))?;
    let Some((_, user)) = queries::find_active_session(&state.db, session_id).await? else {
        return Ok(None);
    };
    if !user.is_active || user.deleted_at.is_some() {
        return Ok(None);
    }
    if touch {
        queries::touch_session(&state.db, session_id).await?;
    }
    Ok(Some((session_id, actor_from_web_user(state, user)?)))
}

pub(crate) async fn current_user_from_bearer(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<crate::actor::ActorContext>, AppError> {
    current_user_from_bearer_with_touch(state, headers, true).await
}

async fn current_user_from_bearer_with_touch(
    state: &AppState,
    headers: &HeaderMap,
    touch: bool,
) -> Result<Option<crate::actor::ActorContext>, AppError> {
    crate::auth::identity::current_actor_from_bearer(state, headers, touch).await
}

pub(crate) async fn current_tool_actor(
    state: &AppState,
    jar: &CookieJar,
    headers: &HeaderMap,
) -> Result<crate::actor::ActorContext, AppError> {
    let cookie_actor = current_user_from_jar_with_touch(state, jar, true).await?;
    let bearer_actor = current_user_from_bearer(state, headers).await?;
    match (cookie_actor, bearer_actor) {
        (Some((_, cookie_actor)), Some(bearer_actor)) => {
            if cookie_actor.principal_id != bearer_actor.principal_id {
                return Err(AppError::unauthorized("conflicting authentication"));
            }
            Ok(cookie_actor)
        }
        (Some((_, actor)), None) | (None, Some(actor)) => Ok(actor),
        (None, None) => Err(AppError::unauthorized("not authenticated")),
    }
}

#[utoipa::path(
    post,
    path = "/api/auth/login",
    tag = "auth",
    request_body = AuthLoginRequest,
    responses(
        (status = 200, body = AuthMeResponse),
        (status = 401, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse)
    )
)]
pub async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    ValidatedJson(request): ValidatedJson<AuthLoginRequest>,
) -> Result<(CookieJar, Json<AuthMeResponse>), AppError> {
    let login_name = request.login_name.trim();

    let Some(user) = queries::find_user_by_login(&state.db, login_name).await? else {
        return Err(AppError::unauthorized("invalid credentials"));
    };
    if !user.is_active || user.deleted_at.is_some() {
        return Err(AppError::forbidden("user is inactive"));
    }
    if !auth::verify_password(&request.password, &user.password_hash)? {
        return Err(AppError::unauthorized("invalid credentials"));
    }

    let session_id = Uuid::new_v4();
    let expires_at = Utc::now()
        + ChronoDuration::from_std(state.config.web_session_ttl)
            .map_err(|_| AppError::bad_request("invalid session ttl"))?;
    queries::create_session(&state.db, session_id, user.user_id, expires_at).await?;
    queries::update_last_login(&state.db, user.user_id).await?;
    queries::record_audit(
        &state.db,
        AuditLogEntry {
            actor_user_id: Some(user.user_id),
            actor_application_id: None,
            actor_type: "user",
            action: "auth.login",
            target_type: "session",
            target_id: &session_id.to_string(),
            workspace_binding_id: None,
            external_user_id: None,
            payload: json!({ "login_name": login_name }),
            request_id: None,
            session_id: None,
            duration_ms: None,
            status: None,
        },
    )
    .await?;

    let cookie = build_session_cookie(
        &state,
        &session_id.to_string(),
        state.config.web_session_ttl.as_secs() as i64,
    );
    let user = queries::find_user_by_id(&state.db, user.user_id)
        .await?
        .ok_or(AppError::unauthorized("invalid credentials"))?;
    let actor = actor_from_web_user(&state, user)?;
    Ok((
        jar.add(cookie),
        Json(AuthMeResponse {
            user: actor
                .user
                .clone()
                .ok_or_else(|| AppError::unauthorized("invalid credentials"))?
                .into(),
        }),
    ))
}

#[utoipa::path(
    post,
    path = "/api/auth/logout",
    tag = "auth",
    responses(
        (status = 204),
        (status = 401, body = crate::error::ErrorResponse)
    )
)]
pub async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<(CookieJar, StatusCode), AppError> {
    let Some((session_id, actor)) = current_user_from_jar(&state, &jar).await? else {
        return Err(AppError::unauthorized("not authenticated"));
    };
    queries::revoke_session(&state.db, session_id).await?;
    queries::record_audit(
        &state.db,
        AuditLogEntry {
            actor_user_id: actor.user.as_ref().map(|user| user.user_id),
            actor_application_id: None,
            actor_type: "user",
            action: "auth.logout",
            target_type: "session",
            target_id: &session_id.to_string(),
            workspace_binding_id: None,
            external_user_id: None,
            payload: json!({}),
            request_id: None,
            session_id: None,
            duration_ms: None,
            status: None,
        },
    )
    .await?;
    let cookie = build_session_cookie(&state, "", 0);
    Ok((jar.add(cookie), StatusCode::NO_CONTENT))
}

#[utoipa::path(
    get,
    path = "/api/auth/me",
    tag = "auth",
    responses(
        (status = 200, body = AuthMeResponse),
        (status = 401, body = crate::error::ErrorResponse)
    )
)]
pub async fn me(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<AuthMeResponse>, AppError> {
    let Some((_, actor)) = current_user_from_jar(&state, &jar).await? else {
        return Err(AppError::unauthorized("not authenticated"));
    };
    Ok(Json(AuthMeResponse {
        user: actor
            .user
            .clone()
            .ok_or_else(|| AppError::unauthorized("not authenticated"))?
            .into(),
    }))
}
