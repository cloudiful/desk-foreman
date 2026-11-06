//! Trusted private-service support for execution environments
//! (issue 749 P2a).
//!
//! Two pieces live here, both purely local to the manager:
//!
//! - [`profile`] parses the optional trusted service configuration. It is
//!   all-or-none and never fatal at startup; an unconfigured environment
//!   behaves exactly as it did before service support existed.
//! - [`credential`] persists one random credential per
//!   `(environment, generation)` under the manager's own state root and
//!   hands the container nothing but an `--env-file` path.
//!
//! This sub-phase only makes the launch prerequisite available. No service
//! is reached, no socket is opened, and no tunnel exists yet.

pub(crate) mod cancel;
pub(crate) mod credential;
pub(crate) mod fs_guard;
pub(crate) mod host_identity;
pub(crate) mod http;
pub(crate) mod http_body;
pub(crate) mod profile;
pub(crate) mod relay;
pub(crate) mod target;

#[cfg(test)]
mod cancel_tests;
#[cfg(test)]
mod guard_tests;
#[cfg(test)]
mod http_limit_tests;
#[cfg(test)]
mod http_target_tests;
#[cfg(test)]
mod http_tests;
#[cfg(test)]
mod launch_tests;
#[cfg(test)]
mod relay_tests;
#[cfg(test)]
mod target_tests;
#[cfg(test)]
mod tests;

pub use cancel::ServiceCancelRegistry;
pub use credential::ServiceCredentialStore;
pub use profile::{ServiceProfile, ServiceProfileConfig};

/// Retire the exact host credential of one confirmed-stopped generation.
///
/// This is the single policy both confirmed stop entry points use. A
/// configured profile validates and unlinks only the credential for this
/// `(environment, generation)`, using the profile's current auth variable
/// name; an unset profile skips retirement and deletes nothing; an invalid
/// profile fails so a configured cleanup problem stays visible. Any error is
/// returned without recreating the credential, and the caller keeps it in the
/// stop outcome so the file lock is still released.
pub(crate) fn retire_confirmed_generation(
    profile: &super::ExecutionEnvironmentProfile,
    state_dir: &std::path::Path,
    environment_id: &str,
    generation: i64,
) -> anyhow::Result<()> {
    let Some(config) = profile.ensure_service_profile()? else {
        return Ok(());
    };
    ServiceCredentialStore::new(state_dir).remove(environment_id, generation, &config.auth_env_name)
}
