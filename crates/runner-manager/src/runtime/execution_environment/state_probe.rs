//! Same-container state writability probe (issue 749 P1).
//!
//! A successful ensure acknowledgment claims more than "the container is
//! running": the trusted image must actually let the configured non-root
//! user write inside the state mount. This probe observes that fact by
//! running a native process *inside the same main container*, as the same
//! non-root user, in the same working directory, so it needs no helper
//! container, no root privileges, no new mount, and no new port.
//!
//! The script is a fixed manager constant. Configured paths and arguments
//! travel as separate Docker argv entries and are never interpolated into
//! the script text, so no request- or config-supplied string can reach the
//! shell. It creates a private `mktemp` file in the working directory,
//! writes to it, removes it, and removes it again on any exit, which is a
//! real create/write/unlink observation rather than a recursive tree
//! walk or an ownership mutation.
//!
//! The probe is an ordinary bounded CLI invocation: it is not claimed to
//! be cancellation-safe if the CLI times out, and neither its exit status
//! nor a CLI timeout is proof that the container stopped. Termination
//! stays owned by the existing stop barrier.

use super::deadline::OperationDeadline;
use super::profile::EnsureProcessProfile;
use crate::runtime::docker::DockerRunnerBackend;

/// Fixed, value-free probe script. `mktemp` keeps the file private and
/// inside the current working directory, which Docker already set to the
/// validated state workdir.
const STATE_PROBE_SCRIPT: &str = "set -eu;probe=$(mktemp ./.df-state-probe.XXXXXX);trap 'rm -f \"$probe\"' EXIT HUP INT TERM;printf 'df' >\"$probe\";rm -f \"$probe\";trap - EXIT HUP INT TERM";

/// Identity of the container and user the probe runs as.
pub struct StateProbeSpec<'a> {
    /// Exact owner-validated container name.
    pub container_name: &'a str,
    /// Validated absolute state working directory.
    pub state_workdir: &'a str,
    /// Non-root uid the probe runs as.
    pub uid: u32,
    /// Group id the probe runs as.
    pub gid: u32,
}

/// Build the `docker exec` argv for the state probe.
///
/// The container name, user, and working directory are separate argv
/// entries, so nothing configured is ever concatenated into the script.
pub fn build_state_probe_argv(spec: &StateProbeSpec<'_>) -> Vec<String> {
    vec![
        "exec".to_string(),
        "--user".to_string(),
        format!("{}:{}", spec.uid, spec.gid),
        "--workdir".to_string(),
        spec.state_workdir.to_string(),
        spec.container_name.to_string(),
        "sh".to_string(),
        "-c".to_string(),
        STATE_PROBE_SCRIPT.to_string(),
    ]
}

/// Confirm real non-root writability in the container state workdir.
///
/// Fails closed: an expired aggregate deadline, a transport failure, or a
/// non-zero exit all become an error, so a caller can never acknowledge a
/// running environment whose state directory was not observed writable.
/// Probe output is never included in the diagnostic, and the failure never
/// stops, removes, or otherwise cleans anything up.
pub(crate) async fn confirm_state_writable(
    backend: &DockerRunnerBackend,
    container_name: &str,
    process: &EnsureProcessProfile<'_>,
    deadline: &OperationDeadline,
) -> anyhow::Result<()> {
    let spec = StateProbeSpec {
        container_name,
        state_workdir: process.state_workdir,
        uid: process.uid,
        gid: process.gid,
    };
    let output = backend
        .docker_output_bounded(build_state_probe_argv(&spec), deadline.remaining_ms()?)
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "execution environment state probe could not be run in {container_name}; refusing to acknowledge a running environment"
            )
        })?;
    if output.timed_out {
        anyhow::bail!(
            "execution environment state probe did not finish within the operation deadline; the container state is uncertain and is not acknowledged as running"
        );
    }
    if output.exit_code != Some(0) {
        anyhow::bail!(
            "execution environment state probe reported the state working directory is not writable by the configured non-root user (exit_code={:?})",
            output.exit_code
        );
    }
    Ok(())
}
