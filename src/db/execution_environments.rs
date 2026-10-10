//! Durable execution environment persistence (issue 737 P1) and authenticated
//! asynchronous lifecycle (issue 742 P3).
//!
//! Thin module entry: focused types, checked persistence, lifecycle
//! orchestration and pure unit tests live in the sibling directory so this
//! file stays wiring-only.

pub mod lifecycle;
pub mod service;
pub mod store;
pub mod tasks;
pub mod types;

#[cfg(test)]
mod tests;

pub use lifecycle::{
    EnvironmentOperationDescriptor, LIFECYCLE_ENVIRONMENT_STATES, LIFECYCLE_STATE_READY,
    LIFECYCLE_STATE_REPLACED, LIFECYCLE_STATE_STARTING, LIFECYCLE_STATE_STOPPING,
    LIFECYCLE_STATE_UNCERTAIN, LifecycleEnvironment, LifecycleError, OperationDescriptor,
    execution_environment_status, provision_execution_environment, replace_execution_environment,
    stop_execution_environment,
};
pub use service::{ServiceEnvironment, find_execution_environment_for_service};
pub use store::{
    EnsureOutcome, InvalidateOutcome, ensure_execution_environment, find_execution_environment,
    find_execution_environment_by_identity, invalidate_execution_environment,
};
pub use tasks::{
    NATIVE_TASK_ACTIVE_STATES, NATIVE_TASK_STATES, NativeTask, NativeTaskError, NativeTaskRequest,
    PreparationDescriptor, PrepareError, PrepareRequest, admit_native_task, cancel_native_task,
    find_native_task, find_native_task_preparation, prepare_native_task, release_native_task,
};
pub use types::{
    EXECUTION_ENVIRONMENT_INITIAL_STATE, EXECUTION_ENVIRONMENT_INVALIDATED_STATE,
    EXECUTION_ENVIRONMENT_STATES, ExecutionEnvironment, derive_state_volume_ref,
    execution_state_is_valid, external_user_id_is_valid, runtime_kind_is_valid,
};
