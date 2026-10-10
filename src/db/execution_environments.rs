//! Durable execution environment persistence (issue 737 P1) and authenticated
//! asynchronous lifecycle (issue 742 P3).
//!
//! Thin module entry: focused types, checked persistence, lifecycle
//! orchestration and pure unit tests live in the sibling directory so this
//! file stays wiring-only.

pub mod lifecycle;
pub mod service;
pub mod store;
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
pub use types::{
    EXECUTION_ENVIRONMENT_INITIAL_STATE, EXECUTION_ENVIRONMENT_INVALIDATED_STATE,
    EXECUTION_ENVIRONMENT_STATES, ExecutionEnvironment, derive_state_volume_ref,
    execution_state_is_valid, external_user_id_is_valid, runtime_kind_is_valid,
};
