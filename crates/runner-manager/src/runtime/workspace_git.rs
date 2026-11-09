//! Fixed paths and sanitized Git configuration for automatic native Git
//! (issue 1180 P28).
//!
//! The capability lives only in `/run/df-git/capability` on a small tmpfs. The
//! helper scripts are static, contain no token, and are invoked through
//! `/bin/sh` because the mount is `noexec`.

pub(crate) const CAPABILITY_PATH: &str = "/run/df-git/capability";
pub(crate) const ASKPASS_PATH: &str = "/run/df-git/askpass";
pub(crate) const CREDENTIAL_HELPER_PATH: &str = "/run/df-git/credential-helper";
pub(crate) const GIT_ASKPASS: &str = "/bin/sh /run/df-git/askpass";
pub(crate) const CREDENTIAL_HELPER: &str = "!/bin/sh /run/df-git/credential-helper";

const MAX_ALIAS_BYTES: usize = 64;
const MAX_URL_BYTES: usize = 2048;

pub(crate) fn askpass_script() -> &'static str {
    "#!/bin/sh\ncase \"$1\" in\n  Username*) printf '%s\\n' df-git ;;\n  Password*) cat /run/df-git/capability ;;\n  *) exit 1 ;;\nesac\n"
}

pub(crate) fn credential_helper_script() -> &'static str {
    "#!/bin/sh\ntest \"$1\" = get || exit 0\nprintf 'username=%s\\n' df-git\nprintf 'password=%s\\n' \"$(cat /run/df-git/capability)\"\n"
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
