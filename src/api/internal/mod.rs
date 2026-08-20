//! Internal API surface.
//!
//! Endpoint implementations are split across focused sibling modules:
//!   * [`lease`]          - acquire/release/takeover/status handlers
//!   * [`runner_manager`] - runner-manager bearer-token endpoints
//!   * [`application`]    - capability introspection and git sync
//!   * [`shared`]         - application actor + binding resolution helpers
//!
//! Each handler module re-exports its `pub` functions so the OpenAPI
//! registration in [`super::super::openapi`] can pick them up without
//! touching the router implementation here.

pub(super) mod application;
pub(super) mod lease;
pub(super) mod runner_manager;
pub(super) mod shared;

pub(super) use application::{application_capabilities, sync_resource_workspace_git};
pub(super) use lease::{
    acquire_resource_workspace_lease, acquire_workspace_binding_lease,
    release_resource_workspace_lease, release_workspace_binding_lease,
    resource_workspace_lease_status, takeover_resource_workspace_lease,
};
pub(super) use runner_manager::{
    complete_runner_job, next_runner_job, report_workspace_runner, runner_manager_config,
};

pub(super) fn router() -> axum::Router<crate::AppState> {
    axum::Router::new()
        .route(
            "/api/internal/application/capabilities",
            axum::routing::get(application_capabilities),
        )
        .route(
            "/api/internal/runner-manager/config",
            axum::routing::get(runner_manager_config),
        )
        .route(
            "/api/internal/runner-manager/jobs/next",
            axum::routing::get(next_runner_job),
        )
        .route(
            "/api/internal/runner-manager/jobs/result",
            axum::routing::post(complete_runner_job),
        )
        .route(
            "/api/internal/runner-manager/workspace-runners/report",
            axum::routing::post(report_workspace_runner),
        )
        .route(
            "/api/internal/resource-workspaces/{resource_kind}/{resource_id}/git/sync",
            axum::routing::post(sync_resource_workspace_git),
        )
        .route(
            "/api/internal/workspace-bindings/{binding_id}/lease",
            axum::routing::post(acquire_workspace_binding_lease)
                .delete(release_workspace_binding_lease),
        )
        .route(
            "/api/internal/resource-workspaces/{resource_kind}/{resource_id}/lease",
            axum::routing::post(acquire_resource_workspace_lease)
                .delete(release_resource_workspace_lease)
                .get(resource_workspace_lease_status),
        )
        .route(
            "/api/internal/resource-workspaces/{resource_kind}/{resource_id}/lease/takeover",
            axum::routing::post(takeover_resource_workspace_lease),
        )
}
