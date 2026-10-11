//! Agent-neutral execution environment primitives (issue 739 P1).
//!
//! Disabled-by-default Docker primitives that create an isolated
//! persistent-state environment and confirm a full container stop. No
//! OpenCode integration, public proxy, routing, or live enablement.
//!
//! Design notes:
//! - Locking is environment-level (`environment_id` only). Generation
//!   locks cannot serialize replacement.
//! - Docker label scans are a guard, not an atomic distributed lock.
//!   Multiple managers or engines still need durable affinity/CAS in a
//!   later lifecycle phase; this module fails closed and never claims
//!   cluster single-writer.
//! - Volume names derive from the environment UUID only, so they stay
//!   stable across generations. Creation requires explicit uid/gid;
//!   guessing is blocked.
//! - Stop confirms only after a successful inspect proves
//!   `exited`/`dead`/`removed` or positively identified absence via the
//!   known `No such container/object` error. Anything else fails closed.
//! - One monotonic operation deadline bounds the environment lock and
//!   every Docker command of an ensure/stop; a zero budget errors before
//!   any work.
//! - Every success acknowledgment and destructive command requires
//!   matching owner/environment/generation labels. Foreign or malformed
//!   labels refuse before any stop/rm.
//! - The durable volume is never removed (`docker volume rm` forbidden).
//! - A dedicated persistent state directory (`state_dir`) holds a durable
//!   generation barrier and a cross-process file lock; both are absent
//!   until explicitly configured and an unconfigured root refuses
//!   environment operations instead of falling back to ephemeral state.
//! - The trusted process command and the explicit state working
//!   directory live in `profile`; no container starts without both, and a
//!   successful acknowledgment additionally requires the same-container
//!   non-root state probe in `state_probe` to have observed a real
//!   create/write/unlink in that directory.

pub mod argv;
#[cfg(test)]
mod argv_tests;
pub(crate) mod deadline;
#[cfg(test)]
mod deadline_tests;
#[cfg(test)]
mod durable_tests;
pub mod ensure;
mod ensure_helpers;
#[cfg(test)]
mod ensure_tests;
pub(crate) mod file_lock;
#[cfg(test)]
mod file_lock_tests;
pub(crate) mod inspect;
#[cfg(test)]
mod owner_tests;
pub mod profile;
#[cfg(test)]
mod profile_tests;
pub mod service;
pub mod state_probe;
#[cfg(test)]
mod state_probe_tests;
pub mod stop;
#[cfg(test)]
mod stop_tests;
#[cfg(test)]
pub(crate) mod test_support;
pub(crate) mod tombstone;
#[cfg(test)]
mod tombstone_tests;

pub use ensure::ensure_execution_environment;
pub use profile::ExecutionEnvironmentProfile;
pub use stop::stop_execution_environment;

use std::time::Duration;

/// Default aggregate budget for one ensure/stop when unset.
pub const DEFAULT_OPERATION_DEADLINE: Duration = Duration::from_secs(60);
/// Lower bound accepted for a configured operation deadline.
pub const MIN_OPERATION_DEADLINE: Duration = Duration::from_secs(1);
/// Upper bound accepted for a configured operation deadline.
pub const MAX_OPERATION_DEADLINE: Duration = Duration::from_secs(3600);

/// Docker label carrying the environment UUID.
pub const LABEL_ENVIRONMENT: &str = "desk-foreman.execution-environment";
/// Docker label carrying the decimal generation.
pub const LABEL_GENERATION: &str = "desk-foreman.generation";
/// Docker label carrying the full owner stable key.
pub const LABEL_OWNER: &str = "desk-foreman.owner";
/// Docker label carrying the manager id.
pub const LABEL_MANAGER: &str = "desk-foreman.manager";
/// Docker label marking DF-managed containers.
pub const LABEL_MANAGED: &str = "desk-foreman.managed";
/// Docker label carrying the native task UUID.
pub const LABEL_NATIVE_TASK_ID: &str = "df.native_task_id";
/// Docker label carrying the bound workspace id.
pub const LABEL_NATIVE_BINDING_ID: &str = "df.native_binding_id";
/// Docker label carrying the sealed task manifest.
pub const LABEL_NATIVE_SEED_SHA256: &str = "df.native_seed_sha256";
/// Docker label carrying the task preparation generation.
pub const LABEL_NATIVE_PREPARATION_GENERATION: &str = "df.native_preparation_generation";
/// Fixed bind destination for a native task checkout.
pub const NATIVE_TASK_WORKDIR: &str = "/workspace";

#[cfg(test)]
mod task_mount_tests;
