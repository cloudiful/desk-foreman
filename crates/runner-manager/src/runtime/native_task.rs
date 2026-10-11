//! Manager-local native-task checkout and execution (issue 764 P2b).
//!
//! Preparation materializes a sealed seed under an explicit local root. Native
//! start mounts only that task tree and its environment state volume; runtime
//! inputs remain manager-owned and no canonical project or `.git` is touched.
//!
//! ## Layout and trust
//!
//! ```text
//! <root>/<environment_uuid>/<task_uuid>/tree        the materialized checkout
//! <root>/<environment_uuid>/<task_uuid>/journal.json its identity and phase
//! ```
//!
//! Both identifiers are trusted UUIDs derived from the durable job, never from
//! a request path or a subject. The root is an explicit
//! `RUNNER_NATIVE_TASK_ROOT`; when it is absent, preparation stays disabled and
//! native start fails closed. Neither path falls back to a workspace or canonical
//! root.
//!
//! The root must be an existing directory owned by this manager's effective uid
//! with an owner-only mode, and its whole chain must contain no symlink. Under
//! that stated assumption — a manager-owned, non-shared root — the per-component
//! `create_new` writes plus handle-validated metadata are safe. Closing the
//! remaining check-then-open window would need `O_NOFOLLOW`, which is not an
//! available workspace dependency; this module says so rather than claiming the
//! window is gone.
//!
//! See [`store`] for materialization and [`execution`] for task-aware start and
//! stop transitions.

mod execution;
mod journal;
mod paths;
mod store;
mod tree;

#[cfg(test)]
mod test_helpers;
#[cfg(test)]
mod test_journal;
#[cfg(test)]
mod tests;

pub(crate) use execution::{export_native_task, start_native_task, stop_native_task};
pub(crate) use store::NativeTaskStore;
