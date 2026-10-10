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

pub mod argv;
mod deadline;
#[cfg(test)]
mod deadline_tests;
#[cfg(test)]
mod durable_tests;
pub mod ensure;
#[cfg(test)]
mod ensure_tests;
mod file_lock;
#[cfg(test)]
mod file_lock_tests;
mod inspect;
#[cfg(test)]
mod owner_tests;
pub mod stop;
#[cfg(test)]
mod stop_tests;
#[cfg(test)]
pub(crate) mod test_support;
mod tombstone;
#[cfg(test)]
mod tombstone_tests;

pub use ensure::ensure_execution_environment;
pub use stop::stop_execution_environment;

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Default aggregate budget for one ensure/stop when unset.
pub const DEFAULT_OPERATION_DEADLINE: Duration = Duration::from_secs(60);
/// Lower bound accepted for a configured operation deadline.
const MIN_OPERATION_DEADLINE: Duration = Duration::from_secs(1);
/// Upper bound accepted for a configured operation deadline.
const MAX_OPERATION_DEADLINE: Duration = Duration::from_secs(3600);

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

/// Trusted manager-level opt-in for environment primitives.
///
/// Uses the existing configured image and resource limits as defaults;
/// image, host paths, volumes, env, and secrets are never accepted from
/// requests. Disabled by default; requests fail closed until explicitly
/// enabled with explicit volume ownership.
#[derive(Clone, Debug)]
pub struct ExecutionEnvironmentProfile {
    pub enabled: bool,
    pub state_mount_target: String,
    pub stop_timeout: Duration,
    /// Aggregate monotonic budget covering the environment lock plus
    /// every Docker command of one ensure/stop. Must be positive.
    pub operation_deadline: Duration,
    pub volume_uid: Option<u32>,
    pub volume_gid: Option<u32>,
    /// Dedicated, persistent, manager-controlled state root. Must not sit
    /// under the agent workspace mount. `None` disables environment
    /// operations entirely; there is no ephemeral fallback.
    pub state_dir: Option<PathBuf>,
}

impl Default for ExecutionEnvironmentProfile {
    fn default() -> Self {
        Self {
            enabled: false,
            state_mount_target: "/state".to_string(),
            stop_timeout: Duration::from_secs(20),
            operation_deadline: DEFAULT_OPERATION_DEADLINE,
            volume_uid: None,
            volume_gid: None,
            state_dir: None,
        }
    }
}

impl ExecutionEnvironmentProfile {
    #[cfg(test)]
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Explicit test opt-in. Real deployments configure uid/gid via
    /// environment; tests pass them directly to avoid global env races.
    #[cfg(test)]
    pub fn test_enabled() -> Self {
        Self {
            enabled: true,
            state_mount_target: "/state".to_string(),
            stop_timeout: Duration::from_millis(500),
            // Short enough to bound the deterministic hang fakes quickly
            // while still far above any fake-CLI spawn latency.
            operation_deadline: Duration::from_secs(1),
            volume_uid: Some(1000),
            volume_gid: Some(1000),
            // Tests must supply an explicit, living state root via
            // `test_enabled_with_state_dir`; a bare disabled/unset root
            // must never silently fall back to ephemeral state.
            state_dir: None,
        }
    }

    /// Explicit test opt-in that also owns a dedicated persistent root.
    #[cfg(test)]
    pub fn test_enabled_with_state_dir(state_dir: impl Into<PathBuf>) -> Self {
        Self {
            state_dir: Some(state_dir.into()),
            ..Self::test_enabled()
        }
    }

    /// Trusted configuration from process env. Disabled unless
    /// `RUNNER_EXECUTION_ENVIRONMENTS_ENABLED=1/true/TRUE`, with explicit
    /// uid/gid or creation stays blocked.
    pub fn from_env_or_default() -> Self {
        let enabled = std::env::var("RUNNER_EXECUTION_ENVIRONMENTS_ENABLED")
            .ok()
            .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "TRUE"));
        let state_mount_target = std::env::var("RUNNER_EXECUTION_ENVIRONMENT_STATE_MOUNT_TARGET")
            .ok()
            .filter(|v| v.starts_with('/'))
            .unwrap_or_else(|| "/state".to_string());
        let stop_timeout = std::env::var("RUNNER_EXECUTION_ENVIRONMENT_STOP_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or_else(|| Duration::from_secs(20));
        let operation_deadline =
            std::env::var("RUNNER_EXECUTION_ENVIRONMENT_OPERATION_DEADLINE_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .map(Duration::from_secs)
                .filter(|value| {
                    *value >= MIN_OPERATION_DEADLINE && *value <= MAX_OPERATION_DEADLINE
                })
                .unwrap_or(DEFAULT_OPERATION_DEADLINE);
        let volume_uid = std::env::var("RUNNER_EXECUTION_ENVIRONMENT_VOLUME_UID")
            .ok()
            .and_then(|v| v.parse::<u32>().ok());
        let volume_gid = std::env::var("RUNNER_EXECUTION_ENVIRONMENT_VOLUME_GID")
            .ok()
            .and_then(|v| v.parse::<u32>().ok());
        // Dedicated persistent state root. Unset/empty stays disabled;
        // environment operations then fail closed before any Docker call.
        let state_dir = std::env::var("RUNNER_EXECUTION_ENVIRONMENT_STATE_DIR")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        Self {
            enabled,
            state_mount_target,
            stop_timeout,
            operation_deadline,
            volume_uid,
            volume_gid,
            state_dir,
        }
    }

    /// Start one bounded operation. Rejects a non-positive deadline
    /// before any lock or Docker work, so a zero budget can never become
    /// an unbounded wait.
    pub(crate) fn operation_deadline(&self) -> anyhow::Result<deadline::OperationDeadline> {
        if self.operation_deadline.is_zero() {
            anyhow::bail!("execution environment operation deadline must be positive");
        }
        Ok(deadline::OperationDeadline::new(self.operation_deadline))
    }

    /// Bounded wait for the graceful `docker stop` command, never zero.
    pub(crate) fn stop_timeout_ms(&self) -> u64 {
        self.stop_timeout.as_millis().max(1) as u64
    }

    pub fn ensure_enabled(&self) -> anyhow::Result<()> {
        if self.enabled {
            Ok(())
        } else {
            anyhow::bail!("execution environments are disabled on this runner manager")
        }
    }

    /// Require a configured dedicated state root. Unset disables
    /// environment operations; there is no ephemeral fallback.
    pub fn ensure_state_dir(&self) -> anyhow::Result<&Path> {
        self.state_dir.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "execution environment persistent state directory is not configured (RUNNER_EXECUTION_ENVIRONMENT_STATE_DIR); refusing to run without durable state"
            )
        })
    }

    pub fn ensure_volume_ownership(&self) -> anyhow::Result<(u32, u32)> {
        match (self.volume_uid, self.volume_gid) {
            (Some(uid), Some(gid)) => Ok((uid, gid)),
            _ => anyhow::bail!(
                "execution environment volume uid/gid are not configured; refusing to guess ownership"
            ),
        }
    }
}
