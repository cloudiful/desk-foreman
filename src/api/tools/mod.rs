pub(super) mod readonly;
pub(super) mod session;

use axum::{Router, http::HeaderMap};
use axum_extra::extract::cookie::CookieJar;

use crate::{
    AppState,
    actor::{ActorContext, actor_from_web_user},
    db::queries,
    error::AppError,
    tools::ToolError,
};

use super::authn::current_tool_actor;

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .merge(session::router())
        .merge(readonly::router())
}

fn map_tool_error(error: ToolError) -> AppError {
    match error {
        ToolError::InvalidInput(message) => AppError::bad_request(message),
        ToolError::NotFound(message) => AppError::not_found(message),
        ToolError::Forbidden(message) => AppError::forbidden(message),
        ToolError::Internal(error) => AppError::internal(error),
    }
}

async fn self_service_actor(
    state: &AppState,
    jar: &CookieJar,
    headers: &HeaderMap,
) -> Result<ActorContext, AppError> {
    current_tool_actor(state, jar, headers).await
}

async fn admin_target_actor(
    state: &AppState,
    jar: &CookieJar,
    headers: &HeaderMap,
    user_id: i64,
) -> Result<ActorContext, AppError> {
    let caller = current_tool_actor(state, jar, headers).await?;
    if !caller.is_admin() {
        return Err(AppError::forbidden("admin access required"));
    }
    let Some(user) = queries::find_user_by_id(&state.db, user_id).await? else {
        return Err(AppError::not_found("user not found"));
    };
    if !user.is_active || user.deleted_at.is_some() {
        return Err(AppError::not_found("user not found"));
    }
    actor_from_web_user(state, user)
}
