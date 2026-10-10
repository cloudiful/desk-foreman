//! Native-task checkout materialization (issue 753 P2b).
//!
//! The original manager materializes one task's sealed seed into a private
//! checkout under an explicit, operator-configured root. This module is
//! **filesystem only**: it starts no container, publishes no port, launches no
//! agent, and never touches a canonical project or its `.git`.
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
//! `RUNNER_NATIVE_TASK_ROOT`; when it is absent, preparation is disabled and
//! every call answers with a fixed unsupported failure, so there is no implicit
//! fallback to a workspace or canonical root.
//!
//! The root must be an existing directory owned by this manager's effective uid
//! with an owner-only mode, and its whole chain must contain no symlink. Under
//! that stated assumption — a manager-owned, non-shared root — the per-component
//! `create_new` writes plus handle-validated metadata are safe. Closing the
//! remaining check-then-open window would need `O_NOFOLLOW`, which is not an
//! available workspace dependency; this module says so rather than claiming the
//! window is gone.
//!
//! See [`store`] for the ordered materialization and [`journal`] for the
//! crash-recovery contract.

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

pub(crate) use store::NativeTaskStore;
