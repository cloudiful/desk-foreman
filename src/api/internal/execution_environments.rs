//! Durable execution environment endpoints (issues 737/742 P3).
//!
//! Thin module entry: wire shapes and authenticated handlers live in the
//! sibling directory so this file stays wiring-only. The `ensure`/`invalidate`
//! metadata handlers keep their P1 semantics; `lifecycle` adds the
//! authenticated asynchronous provision/stop/replace/status routes.

pub mod handlers;
pub mod lifecycle;
pub mod service;
pub mod tasks;
pub mod types;

pub use handlers::{ensure_execution_environment, invalidate_execution_environment};
pub use lifecycle::{
    execution_environment_status, provision_execution_environment, replace_execution_environment,
    stop_execution_environment,
};
pub use service::service_route;
pub use tasks::{
    admit_native_task_handler, cancel_native_task_handler, find_native_task_handler,
    find_native_task_preparation_handler, find_native_task_seed_handler,
    prepare_native_task_handler, release_native_task_handler, seed_native_task_handler,
    start_native_task_handler, stop_native_task_handler,
};
pub use types::{
    EnsureExecutionEnvironmentRequest, InvalidateExecutionEnvironmentRequest,
    ProvisionExecutionEnvironmentRequest, ReplaceExecutionEnvironmentRequest,
    StopExecutionEnvironmentRequest,
};
