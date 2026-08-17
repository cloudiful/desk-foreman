use runner_protocol::CommandOutput;

pub(super) fn is_missing_container_error(message: &str) -> bool {
    message.to_ascii_lowercase().contains("no such object")
}

pub(super) fn ensure_docker_command_succeeded(
    action: &str,
    output: &CommandOutput,
) -> anyhow::Result<()> {
    if output.exit_code == Some(0) && !output.timed_out {
        return Ok(());
    }
    anyhow::bail!(
        "docker failed to {action}: exit_code={:?} timed_out={} output={}",
        output.exit_code,
        output.timed_out,
        output.output.trim()
    )
}

#[cfg(test)]
mod tests {
    use runner_protocol::CommandOutput;

    use super::{ensure_docker_command_succeeded, is_missing_container_error};

    fn command_output(exit_code: Option<i32>, timed_out: bool, output: &str) -> CommandOutput {
        CommandOutput {
            wall_time_seconds: 0.01,
            output: output.to_string(),
            stdout: String::new(),
            stderr: output.to_string(),
            exit_code,
            truncated: false,
            timed_out,
            stdout_bytes: 0,
            stderr_bytes: output.len(),
        }
    }

    #[test]
    fn recognizes_missing_container_across_docker_cli_versions() {
        assert!(is_missing_container_error(
            "Error: No such object: runner-1"
        ));
        assert!(is_missing_container_error(
            "error: no such object: runner-1"
        ));
        assert!(!is_missing_container_error("permission denied"));
    }

    #[test]
    fn rejects_failed_or_timed_out_docker_commands() {
        assert!(
            ensure_docker_command_succeeded("create", &command_output(Some(0), false, "")).is_ok()
        );
        assert!(
            ensure_docker_command_succeeded(
                "create",
                &command_output(Some(1), false, "pull denied")
            )
            .is_err()
        );
        assert!(
            ensure_docker_command_succeeded("create", &command_output(None, true, "")).is_err()
        );
    }
}
