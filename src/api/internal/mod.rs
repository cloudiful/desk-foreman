//! Internal API surface.
//!
//! Endpoint implementations are split across focused sibling modules:
//!   * [`lease`]          - acquire/release/takeover/status handlers
//!   * [`runner_manager`] - runner-manager bearer-token endpoints
//!   * [`application`]    - capability introspection and git sync
//!   * [`execution_environments`] - durable environment ensure/invalidate
//!   * [`shared`]         - application actor + binding resolution helpers
//!
//! Each handler module re-exports its `pub` functions so the OpenAPI
//! registration in [`super::super::openapi`] can pick them up without
//! touching the router implementation here.

pub(super) mod application;
pub(super) mod execution_environments;
pub(super) mod lease;
pub(super) mod lease_audit;
pub(super) mod runner_manager;
pub(super) mod runner_manager_environment_jobs;
pub(super) mod shared;

pub(super) use application::{application_capabilities, sync_resource_workspace_git};
pub(super) use execution_environments::{
    ensure_execution_environment, execution_environment_status, invalidate_execution_environment,
    provision_execution_environment, replace_execution_environment, stop_execution_environment,
};
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
        .route(
            "/api/internal/execution-environments/ensure",
            axum::routing::post(ensure_execution_environment),
        )
        .route(
            "/api/internal/execution-environments/{environment_id}/invalidate",
            axum::routing::post(invalidate_execution_environment),
        )
        .route(
            "/api/internal/execution-environments/{environment_id}/provision",
            axum::routing::post(provision_execution_environment),
        )
        .route(
            "/api/internal/execution-environments/{environment_id}/stop",
            axum::routing::post(stop_execution_environment),
        )
        .route(
            "/api/internal/execution-environments/{environment_id}/replace",
            axum::routing::post(replace_execution_environment),
        )
        .route(
            "/api/internal/execution-environments/{environment_id}/status",
            axum::routing::get(execution_environment_status),
        )
}
