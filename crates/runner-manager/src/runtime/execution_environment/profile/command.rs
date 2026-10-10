//! Trusted container command parsing (issue 749 P1).
//!
//! `RUNNER_EXECUTION_ENVIRONMENT_COMMAND` is a JSON array of argv tokens,
//! parsed with the crate's existing `serde_json` dependency. It is never a
//! shell string: there is no eval and no word splitting, so every token
//! reaches `docker run` with its exact bytes, including embedded spaces
//! and shell metacharacters.
//!
//! A missing or blank value leaves the profile unset — there is no default
//! command — and an explicitly malformed value is retained as `Invalid`
//! rather than silently falling back. Every reason names the setting and
//! the violated rule; the configured value is never echoed back, because a
//! malformed command may carry host details.

/// Upper bound on the number of configured command arguments.
pub const MAX_COMMAND_ARGUMENTS: usize = 64;
/// Upper bound on the total byte size of every configured command argument.
pub const MAX_COMMAND_BYTES: usize = 16 * 1024;

/// Trusted process command as configured by the manager.
///
/// Either absent, retained-as-invalid, or a validated argv. Only the
/// validated form may reach a `docker run`; the raw text is never stored.
#[derive(Clone, Debug, Default)]
pub enum TrustedCommand {
    /// Not configured. There is no default command.
    #[default]
    Unset,
    /// Explicitly malformed. Carries a redacted reason, never the value.
    Invalid(String),
    /// Validated argv; element 0 is the executable.
    Configured(Vec<String>),
}

/// Parse `RUNNER_EXECUTION_ENVIRONMENT_COMMAND` into a trusted command.
///
/// Absent/blank leaves the profile unset; an explicit malformed value
/// becomes `Invalid` instead of a silent default.
pub(crate) fn parse_trusted_command(raw: Option<&str>) -> TrustedCommand {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return TrustedCommand::Unset;
    };
    match serde_json::from_str::<Vec<String>>(raw) {
        Ok(argv) => validate_command_argv(argv),
        Err(_) => TrustedCommand::Invalid(String::from(
            "execution environment command (RUNNER_EXECUTION_ENVIRONMENT_COMMAND) must be a JSON array of string arguments",
        )),
    }
}

fn invalid_command(reason: &'static str) -> TrustedCommand {
    TrustedCommand::Invalid(format!(
        "execution environment command (RUNNER_EXECUTION_ENVIRONMENT_COMMAND) {reason}"
    ))
}

/// Validate a parsed argv. Element 0 must name a non-empty executable;
/// ordinary empty arguments after it stay valid, because a trusted command
/// may legitimately pass one.
fn validate_command_argv(argv: Vec<String>) -> TrustedCommand {
    if argv.is_empty() {
        return invalid_command("must not be an empty array");
    }
    if argv.len() > MAX_COMMAND_ARGUMENTS {
        return invalid_command("must not carry more arguments than the configured bound");
    }
    let total_bytes: usize = argv.iter().map(|argument| argument.len()).sum();
    if total_bytes > MAX_COMMAND_BYTES {
        return invalid_command("must not exceed the configured total size bound");
    }
    if argv.iter().any(|argument| argument.contains('\0')) {
        return invalid_command("must not contain NUL bytes");
    }
    if argv[0].is_empty() {
        return invalid_command("must name a non-empty executable");
    }
    TrustedCommand::Configured(argv)
}
