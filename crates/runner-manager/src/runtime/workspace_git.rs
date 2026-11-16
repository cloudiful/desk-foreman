//! Fixed paths and sanitized Git configuration for automatic native Git
//! (issue 1180 P28).
//!
//! The capability lives only in `/run/df-git/capability` on a small tmpfs. The
//! helper script is static, contains no token, and is invoked through
//! `/bin/sh` because the mount is `noexec`. A companion `allowed-origins` file
//! holds the exact gateway origin for the current dispatch; the helper answers
//! only a matching challenge and stays silent for every other origin.
//!
//! Git never receives a program path for the credential material: the native
//! `credential.helper` is configured at process scope for every shell exec, so
//! even the first clone of a workspace that has no `.git/config` yet resolves
//! the helper before Git could fall back to an askpass program.

use runner_protocol::workspace_git::WorkspaceGitRemote;

pub(crate) const CAPABILITY_PATH: &str = "/run/df-git/capability";
pub(crate) const CREDENTIAL_HELPER_PATH: &str = "/run/df-git/credential-helper";
pub(crate) const ALLOWED_ORIGINS_PATH: &str = "/run/df-git/allowed-origins";
pub(crate) const CREDENTIAL_HELPER: &str = "!/bin/sh /run/df-git/credential-helper";

const MAX_ALIAS_BYTES: usize = 64;
const MAX_URL_BYTES: usize = 2048;
const MAX_ALLOWED_ORIGINS_BYTES: usize = 1024;

/// Static helper body. Only the two fixed tmpfs paths stand between `git` and
/// the capability: a challenge is answered only for an exact trusted origin.
const CREDENTIAL_HELPER_SCRIPT: &str = r#"#!/bin/sh
test "$1" = get || exit 0
protocol=
host=
seen_protocol=0
seen_host=0
while IFS='=' read -r key value; do
    case "$key" in
        "") break ;;
        protocol)
            [ "$seen_protocol" -eq 0 ] || exit 0
            protocol=$value
            seen_protocol=1
            ;;
        host)
            [ "$seen_host" -eq 0 ] || exit 0
            host=$value
            seen_host=1
            ;;
    esac
done
[ "$seen_protocol" -eq 1 ] && [ "$seen_host" -eq 1 ] || exit 0
protocol=$(printf '%s' "$protocol" | tr 'A-Z' 'a-z')
host=$(printf '%s' "$host" | tr 'A-Z' 'a-z')
case "$protocol" in
    http|https) ;;
    *) exit 0 ;;
esac
[ -n "$host" ] || exit 0
[ -r /run/df-git/allowed-origins ] || exit 0
matches() {
    while IFS= read -r line; do
        [ "$line" = "$1" ] && return 0
    done < /run/df-git/allowed-origins
    return 1
}
pair=$(printf '%s\t%s' "$protocol" "$host")
matches "$pair" || exit 0
[ -r /run/df-git/capability ] && [ -s /run/df-git/capability ] || exit 0
printf 'username=%s\n' df-git
printf 'password=%s\n' "$(cat /run/df-git/capability)"
"#;

pub(crate) fn credential_helper_script() -> &'static str {
    CREDENTIAL_HELPER_SCRIPT
}

/// Derives the single trusted gateway origin from the broker URLs the manager
/// already receives for this dispatch. The file holds exact `scheme<TAB>
/// authority` pairs; an explicit default port is accepted as the same origin,
/// other ports, hosts and schemes are not.
pub(crate) fn allowed_origins_metadata(upstreams: &[WorkspaceGitRemote]) -> Option<String> {
    let mut origin: Option<(String, String, Option<u16>)> = None;
    for upstream in upstreams {
        let candidate = canonical_origin(&upstream.url)?;
        match &origin {
            Some(existing) if existing != &candidate => return None,
            Some(_) => {}
            None => origin = Some(candidate),
        }
    }
    let (scheme, host, port) = origin?;
    let authority = match port {
        Some(port) => format!("{host}:{port}"),
        None => host.clone(),
    };
    let mut body = format!("{scheme}\t{authority}\n");
    if port.is_none() {
        let default_port: u16 = if scheme == "https" { 443 } else { 80 };
        body.push_str(&format!("{scheme}\t{host}:{default_port}\n"));
    }
    (body.len() <= MAX_ALLOWED_ORIGINS_BYTES).then_some(body)
}

fn canonical_origin(url: &str) -> Option<(String, String, Option<u16>)> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let scheme = parsed.scheme();
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let default_port: u16 = if scheme == "https" { 443 } else { 80 };
    let port = parsed.port().filter(|port| *port != default_port);
    Some((scheme.to_string(), host, port))
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
