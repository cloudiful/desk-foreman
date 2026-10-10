//! Trusted command and state-workdir validation tests (issue 749 P1,
//! `#[cfg(test)]`).
//!
//! Pure, env-free tests of the two parsing entry points and of the
//! ensure-time resolution gate, so config fidelity and the lexical
//! boundary rules are proven without a daemon or a global env race.

#[cfg(test)]
mod profile_tests {
    use super::super::profile::{
        EnsureProcessProfile, ExecutionEnvironmentProfile,
        command::{
            MAX_COMMAND_ARGUMENTS, MAX_COMMAND_BYTES, TrustedCommand, parse_trusted_command,
        },
        workdir::{TrustedStateWorkdir, parse_state_workdir},
    };
    use std::path::PathBuf;
    use std::time::Duration;

    fn configured(argv: &[&str]) -> TrustedCommand {
        match parse_trusted_command(Some(&serde_json::to_string(argv).expect("serialize argv"))) {
            TrustedCommand::Configured(parsed) => TrustedCommand::Configured(parsed),
            other => other,
        }
    }

    fn argv_of(command: TrustedCommand) -> Option<Vec<String>> {
        match command {
            TrustedCommand::Configured(argv) => Some(argv),
            _ => None,
        }
    }

    #[test]
    fn command_json_keeps_token_bytes_including_spaces_and_empty_arguments() {
        let raw = r#"["/opt/agent","serve","--label","a b c","--empty","","tail -f /dev/null"]"#;
        assert_eq!(
            argv_of(parse_trusted_command(Some(raw))).expect("configured"),
            vec![
                "/opt/agent",
                "serve",
                "--label",
                "a b c",
                "--empty",
                "",
                "tail -f /dev/null",
            ],
            "a JSON array is an argv, never a shell string"
        );
    }

    #[test]
    fn command_never_becomes_a_shell_string_or_interpolates_tokens() {
        let raw = r#"["/opt/agent","run","$(id)","`id`","a;b","*"]"#;
        let argv = argv_of(parse_trusted_command(Some(raw))).expect("configured");
        assert_eq!(argv[2], "$(id)");
        assert_eq!(argv[3], "`id`");
        assert_eq!(argv[4], "a;b");
        assert_eq!(argv[5], "*");
    }

    #[test]
    fn absent_or_blank_command_stays_unset_without_a_default() {
        assert!(matches!(parse_trusted_command(None), TrustedCommand::Unset));
        assert!(matches!(
            parse_trusted_command(Some("   ")),
            TrustedCommand::Unset
        ));
    }

    #[test]
    fn malformed_command_is_retained_as_invalid_and_never_echoed() {
        for raw in [
            "not-json",
            "{}",
            r#"{"command":"sleep"}"#,
            r#"["ok", 7]"#,
            r#"[""  "trailing"]"#,
        ] {
            match parse_trusted_command(Some(raw)) {
                TrustedCommand::Invalid(reason) => {
                    assert!(
                        reason.contains("RUNNER_EXECUTION_ENVIRONMENT_COMMAND"),
                        "reason must name the setting: {reason}"
                    );
                    assert!(
                        !reason.contains(raw),
                        "redacted reason must not echo the configured value"
                    );
                }
                other => panic!("expected invalid, got {other:?}"),
            }
        }
    }

    #[test]
    fn command_bounds_are_enforced() {
        let at_limit: Vec<String> = (0..MAX_COMMAND_ARGUMENTS)
            .map(|index| format!("a{index}"))
            .collect();
        assert!(
            matches!(
                configured(&at_limit.iter().map(String::as_str).collect::<Vec<_>>()),
                TrustedCommand::Configured(_)
            ),
            "the documented argument bound is inclusive"
        );

        let over_limit: Vec<String> = (0..=MAX_COMMAND_ARGUMENTS)
            .map(|index| format!("a{index}"))
            .collect();
        assert!(matches!(
            configured(&over_limit.iter().map(String::as_str).collect::<Vec<_>>()),
            TrustedCommand::Invalid(_)
        ));

        let oversized = vec!["x".repeat(MAX_COMMAND_BYTES + 1)];
        assert!(matches!(
            configured(&oversized.iter().map(String::as_str).collect::<Vec<_>>()),
            TrustedCommand::Invalid(_)
        ));
        let oversized_token = "y".repeat(MAX_COMMAND_BYTES);
        assert!(
            matches!(
                configured(&["/bin/true", oversized_token.as_str()]),
                TrustedCommand::Invalid(_)
            ),
            "the total size bound spans every token, not one token"
        );
    }

    #[test]
    fn command_rejects_nul_and_empty_executable_but_allows_later_empty_arguments() {
        assert!(matches!(
            configured(&["/bin/true", "ok\0bad"]),
            TrustedCommand::Invalid(_)
        ));
        assert!(matches!(
            configured(&["", "infinity"]),
            TrustedCommand::Invalid(_)
        ));
        assert_eq!(
            argv_of(configured(&["/bin/true", ""])).expect("configured"),
            vec!["/bin/true", ""],
            "an ordinary empty argument after the executable stays valid"
        );
    }

    #[test]
    fn state_workdir_accepts_a_strict_descendant() {
        for (raw, expected) in [
            ("/state/work", "/state/work"),
            ("/state/work/agent", "/state/work/agent"),
            ("/state/w", "/state/w"),
        ] {
            match parse_state_workdir(Some(raw), "/state") {
                TrustedStateWorkdir::Configured(path) => assert_eq!(path, expected),
                other => panic!("expected configured for {raw}, got {other:?}"),
            }
        }
    }

    #[test]
    fn state_workdir_rejects_escape_root_and_host_relative_forms() {
        for raw in [
            "/state",           // equal root is not a descendant
            "/stateful/work",   // prefix without a component boundary
            "/other/work",      // outside the mount
            "/state/../etc",    // traversal
            "/state/./work",    // dot component
            "/state//work",     // repeated slash
            "/state/work/",     // trailing slash
            "state/work",       // not absolute
            "/state/wo\u{0}rk", // NUL
            "/state/wo\trk",    // control character
        ] {
            assert!(
                matches!(
                    parse_state_workdir(Some(raw), "/state"),
                    TrustedStateWorkdir::Invalid(_)
                ),
                "{raw:?} must be refused"
            );
        }
    }

    #[test]
    fn state_workdir_absent_stays_unset_and_malformed_is_retained() {
        assert!(matches!(
            parse_state_workdir(None, "/state"),
            TrustedStateWorkdir::Unset
        ));
        assert!(matches!(
            parse_state_workdir(Some(""), "/state"),
            TrustedStateWorkdir::Unset
        ));
        match parse_state_workdir(Some("/state/../etc"), "/state") {
            TrustedStateWorkdir::Invalid(reason) => {
                assert!(reason.contains("RUNNER_EXECUTION_ENVIRONMENT_STATE_WORKDIR"));
                assert!(!reason.contains("/state/../etc"));
            }
            other => panic!("expected invalid, got {other:?}"),
        }
    }

    #[test]
    fn state_workdir_follows_a_configured_mount_target() {
        assert!(
            matches!(
                parse_state_workdir(Some("/data/work"), "/state"),
                TrustedStateWorkdir::Invalid(_)
            ),
            "a descendant of the default root is outside a configured mount"
        );
        assert!(matches!(
            parse_state_workdir(Some("/data/work"), "/data"),
            TrustedStateWorkdir::Configured(_)
        ));
    }

    fn base_profile() -> ExecutionEnvironmentProfile {
        ExecutionEnvironmentProfile {
            enabled: true,
            state_dir: Some(PathBuf::from("/var/lib/df/state")),
            ..super::super::test_support::bare_enabled_profile()
        }
    }

    #[test]
    fn ensure_resolution_binds_command_workdir_and_non_root_identity() {
        let profile = base_profile();
        let EnsureProcessProfile {
            command,
            state_workdir,
            uid,
            gid,
        } = profile
            .ensure_process_profile()
            .expect("configured profile resolves");
        assert_eq!(command, &["/usr/bin/sleep", "infinity"]);
        assert_eq!(state_workdir, "/state/work");
        assert_eq!((uid, gid), (1000, 1000));
    }

    #[test]
    fn default_profile_cannot_resolve_and_never_guesses() {
        let profile = ExecutionEnvironmentProfile {
            state_dir: Some(PathBuf::from("/var/lib/df/state")),
            ..ExecutionEnvironmentProfile::default()
        };
        let error = profile
            .ensure_process_profile()
            .expect_err("default must not resolve");
        assert!(
            error
                .to_string()
                .contains("RUNNER_EXECUTION_ENVIRONMENT_COMMAND")
        );
    }

    #[test]
    fn missing_workdir_refuses_without_a_guess() {
        let profile = ExecutionEnvironmentProfile {
            state_workdir: TrustedStateWorkdir::Unset,
            ..base_profile()
        };
        let error = profile
            .ensure_process_profile()
            .expect_err("missing workdir must refuse");
        assert!(
            error
                .to_string()
                .contains("RUNNER_EXECUTION_ENVIRONMENT_STATE_WORKDIR")
        );
    }

    #[test]
    fn malformed_command_refuses_ensure_with_a_redacted_reason() {
        let profile = ExecutionEnvironmentProfile {
            command: parse_trusted_command(Some("not-json")),
            ..base_profile()
        };
        let error = profile
            .ensure_process_profile()
            .expect_err("malformed command must refuse");
        let message = error.to_string();
        assert!(message.contains("RUNNER_EXECUTION_ENVIRONMENT_COMMAND"));
        assert!(!message.contains("not-json"));
    }

    #[test]
    fn root_ownership_refuses_because_no_agent_or_probe_runs_as_root() {
        let profile = ExecutionEnvironmentProfile {
            volume_uid: Some(0),
            ..base_profile()
        };
        let error = profile
            .ensure_process_profile()
            .expect_err("root uid must refuse");
        assert!(error.to_string().contains("non-root"));
    }

    #[test]
    fn absent_ownership_refuses_before_any_command() {
        let profile = ExecutionEnvironmentProfile {
            volume_uid: None,
            volume_gid: None,
            ..base_profile()
        };
        let error = profile
            .ensure_process_profile()
            .expect_err("absent ownership must refuse");
        assert!(error.to_string().contains("uid/gid are not configured"));
    }

    #[test]
    fn default_profile_stays_disabled_without_explicit_opt_in() {
        let profile = ExecutionEnvironmentProfile::default();
        assert!(!profile.enabled);
        assert!(profile.ensure_enabled().is_err());
        assert!(profile.ensure_state_dir().is_err());
        assert_eq!(profile.state_mount_target, "/state");
        assert_eq!(profile.operation_deadline, Duration::from_secs(60));
    }
}
