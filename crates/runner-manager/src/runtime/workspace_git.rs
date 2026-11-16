//! Fixed paths and sanitized Git configuration for automatic native Git
//! (issue 1180 P28).
//!
//! The capability lives only in `/run/df-git/capability` on a small tmpfs. The
//! helper script is static, contains no token, and is invoked through
//! `/bin/sh` because the mount is `noexec`.
//!
//! Git never receives a program path for the credential material: the native
//! `credential.helper` is configured at process scope for every shell exec, so
//! even the first clone of a workspace that has no `.git/config` yet resolves
//! the helper before Git could fall back to an askpass program.

pub(crate) const CAPABILITY_PATH: &str = "/run/df-git/capability";
pub(crate) const CREDENTIAL_HELPER_PATH: &str = "/run/df-git/credential-helper";
pub(crate) const CREDENTIAL_HELPER: &str = "!/bin/sh /run/df-git/credential-helper";

const MAX_ALIAS_BYTES: usize = 64;
const MAX_URL_BYTES: usize = 2048;

pub(crate) fn credential_helper_script() -> &'static str {
    "#!/bin/sh\ntest \"$1\" = get || exit 0\nprintf 'username=%s\\n' df-git\nprintf 'password=%s\\n' \"$(cat /run/df-git/capability)\"\n"
}

/// Process-scope Git configuration injected into every shell exec.
///
/// The values are static: the capability itself stays in the tmpfs file that
/// the helper reads per dispatch.
pub(crate) fn process_config_env() -> Vec<(&'static str, String)> {
    vec![
        ("GIT_CONFIG_COUNT", "2".to_string()),
        ("GIT_CONFIG_KEY_0", "credential.helper".to_string()),
        ("GIT_CONFIG_VALUE_0", CREDENTIAL_HELPER.to_string()),
        ("GIT_CONFIG_KEY_1", "credential.interactive".to_string()),
        ("GIT_CONFIG_VALUE_1", "false".to_string()),
    ]
}

/// Process environment for Git shells: the process-scope configuration plus an
/// empty `GIT_ASKPASS` and disabled terminal prompts.
///
/// An empty `GIT_ASKPASS` is ignored by Git (it has no program to exec), so it
/// neutralizes a stale askpass value on runners that were created before the
/// helper became the only credential route, and a missing helper then fails
/// closed instead of executing an interpreter-plus-path string.
pub(crate) fn process_git_env() -> Vec<(&'static str, String)> {
    let mut env = process_config_env();
    env.push(("GIT_ASKPASS", String::new()));
    env.push(("GIT_TERMINAL_PROMPT", "0".to_string()));
    env
}

pub(crate) fn valid_alias(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_ALIAS_BYTES
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

pub(crate) fn valid_remote_url(value: &str) -> bool {
    value.len() <= MAX_URL_BYTES
        && !value.bytes().any(|byte| matches!(byte, 0 | b'\r' | b'\n'))
        && value.chars().all(|character| !character.is_control())
        && (value.starts_with("http://") || value.starts_with("https://"))
}

pub(crate) fn remote_config_args(workdir: &str, alias: &str, url: &str) -> Vec<String> {
    vec![
        "-C".to_string(),
        workdir.to_string(),
        "config".to_string(),
        "--local".to_string(),
        format!("remote.{alias}.url"),
        url.to_string(),
    ]
}
