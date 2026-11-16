use super::workspace_git::{
    CAPABILITY_PATH, CREDENTIAL_HELPER, CREDENTIAL_HELPER_PATH, credential_helper_script,
    process_config_env, process_git_env, remote_config_args, valid_alias, valid_remote_url,
};

#[test]
fn helper_script_is_static_and_token_free() {
    let helper = credential_helper_script();
    assert!(helper.contains("/run/df-git/capability"));
    assert!(helper.contains("df-git"));
    assert!(!helper.contains("dfgit1."));
    assert_eq!(CAPABILITY_PATH, "/run/df-git/capability");
    assert_eq!(CREDENTIAL_HELPER_PATH, "/run/df-git/credential-helper");
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
