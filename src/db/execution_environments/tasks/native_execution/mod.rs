mod completion;
#[path = "completion/persistence.rs"]
mod completion_persistence;
#[path = "completion/resolution.rs"]
mod completion_resolution;
mod expiry;
mod identity;
pub(super) mod queries;
pub(super) mod types;

pub(crate) use completion::complete_native_task_job;
pub(crate) use expiry::expire_overdue_native_jobs;
pub(crate) use identity::request_for_native_job;
pub(crate) use types::NativeJobCompletion;
pub use types::{NativeTaskOperationDescriptor, TaskExecutionError, TaskExecutionRequest};
