//! Trusted profile loading from process env (issue 749 P1).
//!
//! Every setting is optional, and loading never fails: a missing optional
//! value leaves the profile unset for ensure and an explicitly malformed
//! value is retained as a degraded profile. An optional configuration
//! mistake therefore can never break manager startup, and there is never a
//! silent fallback to a default that would let a container start without
//! its trusted command or state working directory.
//!
//! The command and working directory are handed to the focused parsers,
//! which own their own bounds and redacted reasons.

use std::path::PathBuf;
use std::time::Duration;

use super::super::service::profile::{ServiceProfileEnv, parse_service_profile};
use super::ExecutionEnvironmentProfile;
use super::command::parse_trusted_command;
use super::workdir::parse_state_workdir;
use crate::runtime::execution_environment::{
    DEFAULT_OPERATION_DEADLINE, MAX_OPERATION_DEADLINE, MIN_OPERATION_DEADLINE,
};

/// Read the whole trusted profile from the process environment.
pub(super) fn load_from_env() -> ExecutionEnvironmentProfile {
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
    let operation_deadline = std::env::var("RUNNER_EXECUTION_ENVIRONMENT_OPERATION_DEADLINE_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .filter(|value| *value >= MIN_OPERATION_DEADLINE && *value <= MAX_OPERATION_DEADLINE)
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
    let command = parse_trusted_command(
        std::env::var("RUNNER_EXECUTION_ENVIRONMENT_COMMAND")
            .ok()
            .as_deref(),
    );
    let state_workdir = parse_state_workdir(
        std::env::var("RUNNER_EXECUTION_ENVIRONMENT_STATE_WORKDIR")
            .ok()
            .as_deref(),
        &state_mount_target,
    );
    // All four private-service settings travel together; a partial set is a
    // degraded profile rather than a half-enabled feature.
    let service = parse_service_profile(ServiceProfileEnv {
        network: std::env::var("RUNNER_EXECUTION_ENVIRONMENT_SERVICE_NETWORK")
            .ok()
            .as_deref(),
        port: std::env::var("RUNNER_EXECUTION_ENVIRONMENT_SERVICE_PORT")
            .ok()
            .as_deref(),
        auth_username: std::env::var("RUNNER_EXECUTION_ENVIRONMENT_SERVICE_AUTH_USERNAME")
            .ok()
            .as_deref(),
        auth_env_name: std::env::var("RUNNER_EXECUTION_ENVIRONMENT_SERVICE_AUTH_ENV_NAME")
            .ok()
            .as_deref(),
    });
    ExecutionEnvironmentProfile {
        enabled,
        state_mount_target,
        stop_timeout,
        operation_deadline,
        volume_uid,
        volume_gid,
        command,
        state_workdir,
        service,
        state_dir,
    }
}
