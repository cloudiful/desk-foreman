//! Durable execution environment endpoints (issues 737/742 P3).
//!
//! Thin module entry: wire shapes and authenticated handlers live in the
//! sibling directory so this file stays wiring-only. The `ensure`/`invalidate`
//! metadata handlers keep their P1 semantics; `lifecycle` adds the
//! authenticated asynchronous provision/stop/replace/status routes.

pub mod handlers;
pub mod lifecycle;
pub mod types;

pub use handlers::{ensure_execution_environment, invalidate_execution_environment};
pub use lifecycle::{
    execution_environment_status, provision_execution_environment, replace_execution_environment,
    stop_execution_environment,
};
pub use types::{
    EnsureExecutionEnvironmentRequest, InvalidateExecutionEnvironmentRequest,
    ProvisionExecutionEnvironmentRequest, ReplaceExecutionEnvironmentRequest,
    StopExecutionEnvironmentRequest,
};
