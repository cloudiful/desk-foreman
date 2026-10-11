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
pub(super) mod runner_manager_service_relay;
pub(super) mod runner_manager_task_exports;
pub(super) mod shared;

pub(super) use application::{application_capabilities, sync_resource_workspace_git};
/// Every relayed method, bound onto the gateway's single route.
use execution_environments::service_route;

pub(super) use execution_environments::tasks::{
    find_native_task_export_content_handler, find_native_task_export_handler,
    schedule_native_task_export_handler,
};
pub(super) use execution_environments::{
    admit_native_task_handler, cancel_native_task_handler, ensure_execution_environment,
    execution_environment_status, find_native_task_handler, find_native_task_preparation_handler,
    find_native_task_seed_handler, invalidate_execution_environment, prepare_native_task_handler,
    provision_execution_environment, release_native_task_handler, replace_execution_environment,
    seed_native_task_handler, start_native_task_handler, stop_execution_environment,
    stop_native_task_handler,
};
pub(super) use lease::{
    acquire_resource_workspace_lease, acquire_workspace_binding_lease,
    release_resource_workspace_lease, release_workspace_binding_lease,
    resource_workspace_lease_status, takeover_resource_workspace_lease,
};
pub(super) use runner_manager::{
    complete_runner_job, next_runner_job, report_workspace_runner, runner_manager_config,
};

pub(super) use runner_manager_environment_jobs::task_seed_for_preparation_job;
pub(super) use runner_manager_service_relay::runner_manager_service_relay;
pub(super) use runner_manager_task_exports::upload_native_task_export_handler;

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
            "/api/internal/runner-managers/{manager_id}/environment-jobs/{job_id}/task-seed",
            axum::routing::get(task_seed_for_preparation_job),
        )
        .route(
            "/api/internal/runner-managers/{manager_id}/environment-jobs/{job_id}/task-export",
            axum::routing::post(upload_native_task_export_handler),
        )
        .route(
            "/api/internal/runner-manager/workspace-runners/report",
            axum::routing::post(report_workspace_runner),
        )
        .route(
            "/api/internal/runner-manager/service-relay",
            axum::routing::get(runner_manager_service_relay),
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
        .route(
            "/api/internal/execution-environments/{environment_id}/tasks/admit",
            axum::routing::post(admit_native_task_handler),
        )
        .route(
            "/api/internal/execution-environments/{environment_id}/tasks/{task_id}",
            axum::routing::get(find_native_task_handler),
        )
        .route(
            "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/cancel",
            axum::routing::post(cancel_native_task_handler),
        )
        .route(
            "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/release",
            axum::routing::post(release_native_task_handler),
        )
        .route(
            "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/start",
            axum::routing::post(start_native_task_handler),
        )
        .route(
            "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/stop",
            axum::routing::post(stop_native_task_handler),
        )
        .route(
            "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/seed",
            axum::routing::post(seed_native_task_handler).get(find_native_task_seed_handler),
        )
        .route(
            "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/prepare",
            axum::routing::post(prepare_native_task_handler)
                .get(find_native_task_preparation_handler),
        )
        .route(
            "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/export",
            axum::routing::post(schedule_native_task_export_handler),
        )
        .route(
            "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/export/status",
            axum::routing::get(find_native_task_export_handler),
        )
        .route(
            "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/export/content",
            axum::routing::get(find_native_task_export_content_handler),
        )
        .route(
            "/api/internal/execution-environments/{environment_id}/service/{*path}",
            service_route(),
        )
}
