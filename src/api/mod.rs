mod admin;
mod authn;
mod internal;
mod openapi;
mod tools;
pub(crate) mod validation;

use axum::{
    Router,
    routing::{get, patch, post},
};

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/auth/login", post(authn::login))
        .route("/api/auth/logout", post(authn::logout))
        .route("/api/auth/me", get(authn::me))
        .route(
            "/api/admin/users",
            get(admin::list_users).post(admin::create_user),
        )
        .route(
            "/api/admin/mcp-tokens",
            get(admin::list_mcp_tokens).post(admin::create_mcp_token),
        )
        .route(
            "/api/admin/applications",
            get(admin::list_applications).post(admin::create_application),
        )
        .route(
            "/api/admin/application-tokens",
            get(admin::list_application_tokens).post(admin::create_application_token),
        )
        .route(
            "/api/admin/workspace-bindings",
            get(admin::list_workspace_bindings),
        )
        .route(
            "/api/admin/users/{user_id}",
            patch(admin::update_user).delete(admin::delete_user),
        )
        .route(
            "/api/admin/applications/{application_id}",
            patch(admin::update_application),
        )
        .route(
            "/api/admin/mcp-tokens/{token_id}",
            axum::routing::delete(admin::delete_mcp_token).patch(admin::update_mcp_token),
        )
        .route(
            "/api/admin/application-tokens/{token_id}",
            axum::routing::delete(admin::delete_application_token)
                .patch(admin::update_application_token),
        )
        .route(
            "/api/admin/workspace-bindings/{binding_id}",
            get(admin::get_workspace_binding),
        )
        .route(
            "/api/admin/workspace-bindings/{binding_id}/archive",
            post(admin::archive_workspace_binding),
        )
        .route(
            "/api/admin/workspace-bindings/{binding_id}/restore",
            post(admin::restore_workspace_binding),
        )
        .route(
            "/api/admin/workspace-bindings/{binding_id}/reset",
            post(admin::reset_workspace_binding),
        )
        .route(
            "/api/admin/workspace-bindings/{binding_id}/lease",
            post(admin::acquire_workspace_binding_lease)
                .delete(admin::release_workspace_binding_lease),
        )
        .route(
            "/api/admin/users/{user_id}/reset-password",
            post(admin::reset_user_password),
        )
        .route("/api/admin/audit-logs", get(admin::list_audit_logs))
        .route(
            "/api/admin/workspace-runners",
            get(admin::list_workspace_runners),
        )
        .route(
            "/api/admin/runner-sessions",
            get(admin::list_runner_sessions),
        )
        .route(
            "/api/admin/operations/summary",
            get(admin::operations_summary),
        )
        .route(
            "/api/admin/approval-settings",
            get(admin::get_approval_settings).patch(admin::update_approval_settings),
        )
        .merge(internal::router())
        .merge(tools::router())
}

pub use openapi::openapi_document;
