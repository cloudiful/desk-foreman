//! Trusted state working directory validation (issue 749 P1).
//!
//! `RUNNER_EXECUTION_ENVIRONMENT_STATE_WORKDIR` names the directory the
//! agent runs in inside the container, and it must be a strict descendant
//! of the configured state mount target. The check is purely lexical and
//! component-aware: the host filesystem is never consulted, because the
//! path describes the container's mount and need not exist on the manager
//! host.
//!
//! A missing value leaves the profile unset — there is no default and no
//! guess — and an explicitly malformed value is retained as `Invalid`
//! rather than silently falling back. Reasons name the setting and the
//! violated rule; the configured value is never echoed back.

/// Explicit absolute working directory inside the state mount.
#[derive(Clone, Debug, Default)]
pub enum TrustedStateWorkdir {
    /// Not configured. There is no default working directory.
    #[default]
    Unset,
    /// Explicitly malformed. Carries a redacted reason, never the value.
    Invalid(String),
    /// Validated absolute path strictly below the state mount target.
    Configured(String),
}

/// Parse and lexically validate `RUNNER_EXECUTION_ENVIRONMENT_STATE_WORKDIR`
/// against the configured state mount target.
pub(crate) fn parse_state_workdir(
    raw: Option<&str>,
    state_mount_target: &str,
) -> TrustedStateWorkdir {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return TrustedStateWorkdir::Unset;
    };
    match validate_state_workdir(raw, state_mount_target) {
        Ok(path) => TrustedStateWorkdir::Configured(path),
        Err(reason) => TrustedStateWorkdir::Invalid(format!(
            "execution environment state working directory (RUNNER_EXECUTION_ENVIRONMENT_STATE_WORKDIR) {reason}"
        )),
    }
}

/// Lexically validate one working directory against the state mount root.
///
/// The path must be absolute, free of control characters, free of
/// `.`/`..`/repeated-slash noise, free of a trailing slash, and strictly
/// below the root at a component boundary (so `/stateful` is not inside
/// `/state`).
fn validate_state_workdir(raw: &str, state_mount_target: &str) -> Result<String, &'static str> {
    if !raw.starts_with('/') {
        return Err("must be an absolute path");
    }
    if raw.chars().any(|c| c.is_control()) {
        return Err("must not contain control characters");
    }
    if raw.ends_with('/') {
        return Err("must not end with a trailing slash");
    }
    if raw
        .split('/')
        .any(|component| component == "." || component == "..")
    {
        return Err("must not contain '.' or '..' components");
    }
    // The leading slash is the absolute-path marker, not a repeated one.
    if raw.split('/').skip(1).any(str::is_empty) {
        return Err("must not contain repeated slashes");
    }
    if !is_strict_descendant(state_mount_target, raw) {
        return Err("must be a strict descendant of the configured state mount target");
    }
    Ok(raw.to_string())
}

/// Component-wise strict descendant test. Both sides are compared as
/// `/`-separated components, so a shared prefix is not mistaken for
/// containment.
fn is_strict_descendant(root: &str, candidate: &str) -> bool {
    let root_components: Vec<&str> = root.split('/').filter(|c| !c.is_empty()).collect();
    let candidate_components: Vec<&str> = candidate.split('/').filter(|c| !c.is_empty()).collect();
    candidate_components.len() > root_components.len()
        && candidate_components[..root_components.len()] == root_components[..]
}
