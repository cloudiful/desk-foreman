//! Secured-operation expectations for the OpenAPI security tests
//! (issue 753 P2b).
//!
//! Split from `openapi_security_tests.rs` so no file exceeds the size budget:
//! the flat `(path, method, operation id)` table is data, not logic.

pub(super) const APPLICATION_BEARER_OPERATIONS: [(&str, &str, &str); 21] = [
    (
        "/api/internal/execution-environments/ensure",
        "post",
        "ensure_execution_environment",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/invalidate",
        "post",
        "invalidate_execution_environment",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/provision",
        "post",
        "provision_execution_environment",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/stop",
        "post",
        "stop_execution_environment",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/replace",
        "post",
        "replace_execution_environment",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/status",
        "get",
        "execution_environment_status",
    ),
    // The private-service gateway resolves the same application identity from
    // the same bearer guard, so every relayed method is guarded by it too.
    (
        "/api/internal/execution-environments/{environment_id}/service/{path}",
        "get",
        "execution_environment_service_get",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/service/{path}",
        "post",
        "execution_environment_service_post",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/service/{path}",
        "put",
        "execution_environment_service_put",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/service/{path}",
        "patch",
        "execution_environment_service_patch",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/service/{path}",
        "delete",
        "execution_environment_service_delete",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/service/{path}",
        "head",
        "execution_environment_service_head",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/service/{path}",
        "options",
        "execution_environment_service_options",
    ),
    // Durable native task intents resolve the same application identity from
    // the same bearer guard, so all four task operations are guarded too.
    (
        "/api/internal/execution-environments/{environment_id}/tasks/admit",
        "post",
        "admit_native_task_handler",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/tasks/{task_id}",
        "get",
        "find_native_task_handler",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/cancel",
        "post",
        "cancel_native_task_handler",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/release",
        "post",
        "release_native_task_handler",
    ),
    // The immutable input seed is the same authenticated application surface.
    (
        "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/seed",
        "post",
        "seed_native_task_handler",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/seed",
        "get",
        "find_native_task_seed_handler",
    ),
    // Native-task preparation resolves the same application identity.
    (
        "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/prepare",
        "post",
        "prepare_native_task_handler",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/tasks/{task_id}/prepare",
        "get",
        "find_native_task_preparation_handler",
    ),
];
