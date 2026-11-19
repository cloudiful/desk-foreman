use axum_extra::extract::cookie::CookieJar;

use crate::{
    AppState, actor::ActorContext, authz::ControlAction, db::audit::AuditLogEntry, error::AppError,
};

use super::super::authn::current_user_from_jar;

pub async fn require_admin(
    state: &AppState,
    jar: &CookieJar,
    action: ControlAction,
) -> Result<ActorContext, AppError> {
    let Some((_, actor)) = current_user_from_jar(state, jar).await? else {
        return Err(AppError::unauthorized("not authenticated"));
    };
    ensure_control_action(state, &actor, action)?;
    if actor
        .user
        .as_ref()
        .is_some_and(|user| user.must_change_password)
    {
        return Err(AppError::forbidden("password change required"));
    }
    Ok(actor)
}

/// Applies the shared-engine control decision to an already verified actor.
///
/// The decision uses the actor's current user record and does not consult
/// workspace token or server ceilings, matching the existing control-plane
/// behavior.
pub fn ensure_control_action(
    state: &AppState,
    actor: &ActorContext,
    action: ControlAction,
) -> Result<(), AppError> {
    let Some(user) = actor.user.as_ref() else {
        return Err(AppError::forbidden("admin access required"));
    };
    if !crate::authz::check(
        state.authz.as_ref(),
        crate::authz::Operation::Control { user, action },
    )
    .is_allowed()
    {
        return Err(AppError::forbidden("admin access required"));
    }
    Ok(())
}

pub async fn record_admin_audit(
    state: &AppState,
    admin: &ActorContext,
    action: &'static str,
    target_type: &'static str,
    target_id: String,
    payload: serde_json::Value,
) -> Result<(), AppError> {
    crate::db::queries::record_audit(
        &state.db,
        AuditLogEntry {
            actor_user_id: admin.user.as_ref().map(|user| user.user_id),
            actor_application_id: None,
            actor_type: "user",
            action,
            target_type,
            target_id: &target_id,
            workspace_binding_id: None,
            external_user_id: None,
            payload,
            request_id: None,
            session_id: None,
            duration_ms: None,
            status: None,
        },
    )
    .await?;
    Ok(())
}

pub fn map_db_conflict(error: anyhow::Error) -> AppError {
    let text = error.to_string();
    if text.contains("users_login_name_active_key") || text.contains("users_email_active_key") {
        AppError::conflict("login_name or email already exists")
    } else if text.contains("applications_name_key") {
        AppError::conflict("application name already exists")
    } else {
        AppError::internal(error)
    }
}
