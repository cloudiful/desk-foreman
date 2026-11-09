use super::workspace_git::{
    ASKPASS_PATH, CAPABILITY_PATH, CREDENTIAL_HELPER_PATH, askpass_script,
    credential_helper_script, remote_config_args, valid_alias, valid_remote_url,
};

#[test]
fn helper_scripts_are_static_and_token_free() {
    let askpass = askpass_script();
    let helper = credential_helper_script();
    assert!(askpass.contains("/run/df-git/capability"));
    assert!(helper.contains("/run/df-git/capability"));
    assert!(askpass.contains("df-git"));
    assert!(helper.contains("df-git"));
    assert!(!askpass.contains("dfgit1."));
    assert!(!helper.contains("dfgit1."));
    assert_eq!(CAPABILITY_PATH, "/run/df-git/capability");
    assert_eq!(ASKPASS_PATH, "/run/df-git/askpass");
    assert_eq!(CREDENTIAL_HELPER_PATH, "/run/df-git/credential-helper");
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
