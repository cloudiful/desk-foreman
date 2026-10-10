//! Trusted agent process profile (issue 749 P1).
//!
//! Owns the manager-local trusted configuration that decides *what* an
//! execution environment runs and *where*: the process command argv and
//! the explicit state working directory. Configuration is optional and
//! never fatal: a missing optional value leaves the profile unusable for
//! ensure, and an explicitly malformed value is retained as a degraded
//! profile instead of silently falling back to a default.
//!
//! The three focused responsibilities live beside this module:
//! `command` parses and bounds the trusted argv, `workdir` lexically
//! validates the state working directory, and `loading` reads the process
//! environment. This module holds the profile itself and the ensure-time
//! resolution gate that turns a stored configuration into the exact
//! process inputs a container may run with. Diagnostics name the offending
//! setting and the violated rule, never the configured value.

pub(crate) mod command;
pub(crate) mod loading;
pub(crate) mod workdir;

use std::path::PathBuf;
use std::time::Duration;

use super::DEFAULT_OPERATION_DEADLINE;
use super::service::ServiceProfile;
use command::TrustedCommand;
use workdir::TrustedStateWorkdir;

/// Resolved ensure-time process inputs, all borrowed from the profile.
///
/// `Debug` is redacted on purpose: the configured command can carry host
/// details, so it must never reach a log line through a derived format.
pub struct EnsureProcessProfile<'a> {
    /// Validated container command argv, appended verbatim as `CMD`.
    pub command: &'a [String],
    /// Validated absolute working directory inside the state mount.
    pub state_workdir: &'a str,
    /// Non-root uid the agent and the state probe run as.
    pub uid: u32,
    /// Group id the agent and the state probe run as.
    pub gid: u32,
}

impl std::fmt::Debug for EnsureProcessProfile<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EnsureProcessProfile")
            .field("command", &"<redacted>")
            .field("state_workdir", &self.state_workdir)
            .field("uid", &self.uid)
            .field("gid", &self.gid)
            .finish()
    }
}

/// Trusted manager-level opt-in for environment primitives.
///
/// Uses the existing configured image and resource limits as defaults;
/// image, host paths, volumes, env, secrets, and the executable are never
/// accepted from requests. Disabled by default; requests fail closed
/// until explicitly enabled with explicit volume ownership and an
/// explicit trusted command plus state working directory.
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
    /// Trusted container command argv. Unset or malformed blocks ensure;
    /// there is no default command.
    pub command: TrustedCommand,
    /// Trusted absolute working directory inside the state mount. Unset or
    /// malformed blocks ensure; there is no default and no guess.
    pub state_workdir: TrustedStateWorkdir,
    /// Optional private-service launch configuration. Unset leaves the
    /// environment exactly as it was before service support; malformed is
    /// refused before any Docker work rather than silently degraded.
    pub service: ServiceProfile,
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
            command: TrustedCommand::default(),
            state_workdir: TrustedStateWorkdir::default(),
            service: ServiceProfile::default(),
            state_dir: None,
        }
    }
}

impl ExecutionEnvironmentProfile {
    /// Trusted configuration from process env. Disabled unless
    /// `RUNNER_EXECUTION_ENVIRONMENTS_ENABLED=1/true/TRUE`, with explicit
    /// uid/gid or creation stays blocked.
    pub fn from_env_or_default() -> Self {
        loading::load_from_env()
    }

    /// Resolve the ensure-time process inputs, refusing before any disk or
    /// Docker work. Stop deliberately does not call this, so an existing
    /// environment stays stoppable with an unusable creation profile.
    pub(crate) fn ensure_process_profile(&self) -> anyhow::Result<EnsureProcessProfile<'_>> {
        let command = match &self.command {
            TrustedCommand::Configured(argv) => argv.as_slice(),
            TrustedCommand::Unset => anyhow::bail!(
                "execution environment command (RUNNER_EXECUTION_ENVIRONMENT_COMMAND) is not configured; refusing to start a container without a trusted process command"
            ),
            TrustedCommand::Invalid(reason) => anyhow::bail!("{reason}"),
        };
        let state_workdir = match &self.state_workdir {
            TrustedStateWorkdir::Configured(path) => path.as_str(),
            TrustedStateWorkdir::Unset => anyhow::bail!(
                "execution environment state working directory (RUNNER_EXECUTION_ENVIRONMENT_STATE_WORKDIR) is not configured; refusing to guess a working directory inside the state mount"
            ),
            TrustedStateWorkdir::Invalid(reason) => anyhow::bail!("{reason}"),
        };
        let (uid, gid) = self.ensure_volume_ownership()?;
        if uid == 0 {
            anyhow::bail!(
                "execution environment volume uid must be a non-root user; refusing to run the agent or the state probe as root"
            );
        }
        Ok(EnsureProcessProfile {
            command,
            state_workdir,
            uid,
            gid,
        })
    }

    /// Start one bounded operation. Rejects a non-positive deadline
    /// before any lock or Docker work, so a zero budget can never become
    /// an unbounded wait.
    pub(crate) fn operation_deadline(&self) -> anyhow::Result<super::deadline::OperationDeadline> {
        if self.operation_deadline.is_zero() {
            anyhow::bail!("execution environment operation deadline must be positive");
        }
        Ok(super::deadline::OperationDeadline::new(
            self.operation_deadline,
        ))
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
    pub fn ensure_state_dir(&self) -> anyhow::Result<&std::path::Path> {
        self.state_dir.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "execution environment persistent state directory is not configured (RUNNER_EXECUTION_ENVIRONMENT_STATE_DIR); refusing to run without durable state"
            )
        })
    }

    /// Resolve the optional private-service launch configuration.
    ///
    /// `Ok(None)` means service support is off and the environment is
    /// launched exactly as it was before this feature existed. A degraded
    /// profile is refused rather than treated as "off", because an operator
    /// who set some of the settings expects a serving environment and must
    /// not silently get one that serves nothing.
    pub(crate) fn ensure_service_profile(
        &self,
    ) -> anyhow::Result<Option<&super::service::ServiceProfileConfig>> {
        match &self.service {
            ServiceProfile::Unset => Ok(None),
            ServiceProfile::Configured(config) => Ok(Some(config)),
            ServiceProfile::Invalid(reason) => anyhow::bail!("{reason}"),
        }
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
