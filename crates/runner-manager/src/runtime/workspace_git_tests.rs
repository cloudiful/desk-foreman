use super::workspace_git::{
    ALLOWED_ORIGINS_PATH, CAPABILITY_PATH, CREDENTIAL_HELPER, CREDENTIAL_HELPER_PATH,
    allowed_origins_metadata, credential_helper_script, process_config_env, process_git_env,
    remote_config_args, valid_alias, valid_remote_url,
};
use runner_protocol::workspace_git::WorkspaceGitRemote;

fn remote(url: &str) -> WorkspaceGitRemote {
    WorkspaceGitRemote {
        remote_alias: "origin".to_string(),
        url: url.to_string(),
    }
}

#[test]
fn helper_script_is_static_and_token_free() {
    let helper = credential_helper_script();
    assert!(helper.contains("/run/df-git/capability"));
    assert!(helper.contains("/run/df-git/allowed-origins"));
    assert!(helper.contains("df-git"));
    assert!(!helper.contains("dfgit1."));
    assert_eq!(CAPABILITY_PATH, "/run/df-git/capability");
    assert_eq!(CREDENTIAL_HELPER_PATH, "/run/df-git/credential-helper");
    assert_eq!(ALLOWED_ORIGINS_PATH, "/run/df-git/allowed-origins");
}

#[test]
fn helper_guards_the_origin_before_reading_the_capability() {
    let helper = credential_helper_script();
    assert!(helper.starts_with("#!/bin/sh\n"));
    assert!(helper.contains("test \"$1\" = get || exit 0"));
    assert!(helper.contains("read -r key value"));
    assert!(helper.contains("tr 'A-Z' 'a-z'"));
    assert!(!helper.contains("eval"));
    assert!(!helper.contains("sh -c"));
    assert!(!helper.contains("curl"));
    let scope = helper.find("allowed-origins").unwrap();
    let secret = helper.find("capability").unwrap();
    assert!(
        scope < secret,
        "the helper must match the origin before touching the capability"
    );
}

#[test]
fn default_port_forms_share_one_canonical_origin() {
    let metadata = allowed_origins_metadata(&[
        remote("https://DF.Example.com/git/7/origin"),
        remote("https://df.example.com:443/git/7/upstream"),
    ])
    .unwrap();
    assert_eq!(
        metadata,
        "https\tdf.example.com\nhttps\tdf.example.com:443\n"
    );

    let metadata = allowed_origins_metadata(&[remote("http://127.0.0.1:80/git/7/origin")]).unwrap();
    assert_eq!(metadata, "http\t127.0.0.1\nhttp\t127.0.0.1:80\n");
}

#[test]
fn nondefault_port_and_ipv6_brackets_are_preserved() {
    let metadata =
        allowed_origins_metadata(&[remote("http://127.0.0.1:3000/git/7/origin")]).unwrap();
    assert_eq!(metadata, "http\t127.0.0.1:3000\n");

    let metadata = allowed_origins_metadata(&[remote("https://[::1]:8443/git/7/origin")]).unwrap();
    assert_eq!(metadata, "https\t[::1]:8443\n");

    let metadata = allowed_origins_metadata(&[remote("http://[::1]/git/7/origin")]).unwrap();
    assert_eq!(metadata, "http\t[::1]\nhttp\t[::1]:80\n");
}

#[test]
fn invalid_or_mixed_origins_fail_closed() {
    assert!(
        allowed_origins_metadata(&[remote("https://user@df.example.com/git/7/origin")]).is_none()
    );
    assert!(
        allowed_origins_metadata(&[remote("https://df.example.com/git/7/origin?token=1")])
            .is_none()
    );
    assert!(
        allowed_origins_metadata(&[remote("https://df.example.com/git/7/origin#frag")]).is_none()
    );
    assert!(allowed_origins_metadata(&[remote("ftp://df.example.com/git/7/origin")]).is_none());
    assert!(
        allowed_origins_metadata(&[
            remote("https://df.example.com/git/7/origin"),
            remote("https://other.example.com/git/7/origin"),
        ])
        .is_none(),
        "aliases with different origins must not share one trusted scope"
    );
}

#[test]
fn missing_scope_yields_no_helper_credentials() {
    assert!(allowed_origins_metadata(&[]).is_none());
}

#[test]
fn process_config_uses_the_native_helper_without_a_token() {
    let env = process_config_env();
    assert_eq!(
        env,
        vec![
            ("GIT_CONFIG_COUNT", "2".to_string()),
            ("GIT_CONFIG_KEY_0", "credential.helper".to_string()),
            ("GIT_CONFIG_VALUE_0", CREDENTIAL_HELPER.to_string()),
            ("GIT_CONFIG_KEY_1", "credential.interactive".to_string()),
            ("GIT_CONFIG_VALUE_1", "false".to_string()),
        ]
    );
    assert_eq!(CREDENTIAL_HELPER, "!/bin/sh /run/df-git/credential-helper");
    let rendered: String = env
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!rendered.contains("dfgit1."));
    assert!(!rendered.contains("askpass"));
}

#[test]
fn process_env_empties_askpass_and_disables_prompts() {
    let env = process_git_env();
    let askpass = env
        .iter()
        .find(|(key, _)| *key == "GIT_ASKPASS")
        .map(|(_, value)| value.as_str());
    assert_eq!(askpass, Some(""));
    let prompt = env
        .iter()
        .find(|(key, _)| *key == "GIT_TERMINAL_PROMPT")
        .map(|(_, value)| value.as_str());
    assert_eq!(prompt, Some("0"));
    assert_eq!(
        env.iter().filter(|(key, _)| *key == "GIT_ASKPASS").count(),
        1
    );
    assert!(
        !env.iter()
            .any(|(key, value)| key.contains("ASKPASS") && !value.is_empty())
    );
}

#[test]
fn remote_config_argv_keeps_metadata_out_of_shell_interpolation() {
    let args = remote_config_args("/workspace", "origin", "http://127.0.0.1:3000/git/7/origin");
    assert_eq!(
        args,
        vec![
            "-C",
            "/workspace",
            "config",
            "--local",
            "remote.origin.url",
            "http://127.0.0.1:3000/git/7/origin",
        ]
    );
    assert!(!args.iter().any(|argument| argument.contains("dfgit1.")));
}

#[test]
fn rejects_unsafe_aliases_and_urls() {
    assert!(valid_alias("origin"));
    assert!(valid_alias("up_stream-2"));
    assert!(!valid_alias("../origin"));
    assert!(!valid_alias("has space"));
    assert!(!valid_alias(&"a".repeat(65)));

    assert!(valid_remote_url("https://git.example.com/team/repo.git"));
    assert!(valid_remote_url("http://127.0.0.1:3000/git/7/origin"));
    assert!(!valid_remote_url("ssh://git.example.com/repo.git"));
    assert!(!valid_remote_url("file:///tmp/repo.git"));
    assert!(!valid_remote_url("http://host/repo\n.git"));
}
