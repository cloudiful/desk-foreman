//! Private-service profile and credential tests (issue 749 P2a,
//! `#[cfg(test)]`).
//!
//! Covers the all-or-none profile rule, the argv authority of the network
//! and env-file, and the credential's stability, rotation, and refusal of
//! anything that fails a trust check. Filesystem only: no Docker, no
//! network.

#[cfg(test)]
mod service_tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use super::super::credential::ServiceCredentialStore;
    use super::super::profile::{ServiceProfile, ServiceProfileEnv, parse_service_profile};

    const ENV: &str = "123e4567-e89b-12d3-a456-426614174000";
    const ENV_NAME: &str = "DF_AGENT_TOKEN";

    fn env_name() -> String {
        ENV.chars().filter(|c| *c != '-').collect()
    }

    fn all() -> ServiceProfileEnv<'static> {
        ServiceProfileEnv {
            network: Some("df-private"),
            port: Some("8443"),
            auth_username: Some("agent"),
            auth_env_name: Some(ENV_NAME),
        }
    }

    fn temp() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).expect("read credential")
    }

    fn token_of(path: &Path) -> String {
        read(path)
            .trim_end()
            .split_once('=')
            .expect("NAME=token")
            .1
            .to_string()
    }

    #[test]
    fn absent_profile_leaves_service_off() {
        assert_eq!(
            parse_service_profile(ServiceProfileEnv::default()),
            ServiceProfile::Unset
        );
    }

    #[test]
    fn all_four_settings_yield_a_validated_profile() {
        let ServiceProfile::Configured(config) = parse_service_profile(all()) else {
            panic!("all four settings must configure the profile")
        };
        assert_eq!(config.network, "df-private");
        assert_eq!(config.port, 8443);
        assert_eq!(config.auth_username, "agent");
        assert_eq!(config.auth_env_name, ENV_NAME);
    }

    #[test]
    fn a_partial_configuration_is_refused_rather_than_half_enabled() {
        for missing in 0..4 {
            let mut raw = all();
            match missing {
                0 => raw.network = None,
                1 => raw.port = None,
                2 => raw.auth_username = None,
                _ => raw.auth_env_name = None,
            }
            let ServiceProfile::Invalid(reason) = parse_service_profile(raw) else {
                panic!("missing setting {missing} must be refused")
            };
            assert!(
                reason.contains("configured together"),
                "reason must name the rule, got: {reason}"
            );
            assert!(
                !reason.contains("df-private"),
                "reason must not echo the configured value"
            );
        }
    }

    #[test]
    fn individual_settings_are_validated() {
        for (field, bad) in [
            ("network", ""),
            ("network", "-leading-dash"),
            ("network", "has space"),
            ("network", "has/slash"),
            ("port", "0"),
            ("port", "65536"),
            ("port", "http"),
            ("port", ""),
            ("auth_username", ""),
            ("auth_username", "has:colon"),
            ("auth_username", "has\nnewline"),
            ("auth_env_name", ""),
            ("auth_env_name", "1LEADING_DIGIT"),
            ("auth_env_name", "HAS-DASH"),
        ] {
            let mut raw = all();
            match field {
                "network" => raw.network = Some(bad),
                "port" => raw.port = Some(bad),
                "auth_username" => raw.auth_username = Some(bad),
                _ => raw.auth_env_name = Some(bad),
            }
            assert!(
                matches!(parse_service_profile(raw), ServiceProfile::Invalid(_)),
                "{field}={bad:?} must be refused"
            );
        }
    }

    #[test]
    fn credential_is_stable_per_generation_and_rotates_across_them() {
        let temp = temp();
        let store = ServiceCredentialStore::new(temp.path());
        let first = store.obtain(ENV, 1, ENV_NAME).expect("create");
        let again = store.obtain(ENV, 1, ENV_NAME).expect("reuse");
        assert_eq!(first, again, "the same generation keeps one credential");
        assert_eq!(token_of(first.env_file()), token_of(again.env_file()));

        let rotated = store.obtain(ENV, 2, ENV_NAME).expect("new generation");
        assert_ne!(
            first.env_file(),
            rotated.env_file(),
            "each generation has its own file"
        );
        assert_ne!(
            token_of(first.env_file()),
            token_of(rotated.env_file()),
            "a new generation must not reuse a token"
        );
    }

    #[test]
    fn credentials_are_distinct_per_environment() {
        let temp = temp();
        let store = ServiceCredentialStore::new(temp.path());
        let other = "00000000-0000-0000-0000-000000000000";
        let mine = store.obtain(ENV, 0, ENV_NAME).expect("create");
        let theirs = store.obtain(other, 0, ENV_NAME).expect("create");
        assert_ne!(mine.env_file(), theirs.env_file());
        assert_ne!(
            token_of(mine.env_file()),
            token_of(theirs.env_file()),
            "two users must never share one credential"
        );
    }

    #[test]
    fn credential_file_is_owner_only_and_named_after_the_profile() {
        let temp = temp();
        let store = ServiceCredentialStore::new(temp.path());
        let credential = store.obtain(ENV, 3, ENV_NAME).expect("create");
        let path = credential.env_file();

        let mode = fs::metadata(path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the credential must be owner-only");
        let contents = read(path);
        assert!(
            contents.starts_with(&format!("{ENV_NAME}=")),
            "the file must carry the trusted variable name"
        );
        assert_eq!(token_of(path).len(), 36, "a UUIDv4 token");
        assert!(
            path.starts_with(temp.path()),
            "the credential stays under the manager state root"
        );
        assert_eq!(
            path.parent().and_then(Path::file_name),
            Some(std::ffi::OsStr::new("service")),
            "credentials live in their own directory beside the durable barrier"
        );
        assert_eq!(
            path.parent()
                .and_then(Path::parent)
                .and_then(Path::file_name),
            Some(std::ffi::OsStr::new(&env_name())),
            "the per-environment directory matches the barrier layout"
        );
    }

    #[test]
    fn opening_a_created_credential_again_validates_it() {
        let temp = temp();
        let store = ServiceCredentialStore::new(temp.path());
        let created = store.obtain(ENV, 4, ENV_NAME).expect("create");
        let reopened = store
            .existing(ENV, 4, ENV_NAME)
            .expect("reopen an intact credential");
        assert_eq!(created, reopened);
    }

    #[test]
    fn a_missing_credential_is_refused_rather_than_invented() {
        let temp = temp();
        let store = ServiceCredentialStore::new(temp.path());
        let error = store
            .existing(ENV, 9, ENV_NAME)
            .expect_err("an absent credential must fail closed");
        assert!(error.to_string().contains("not available"), "got: {error}");
    }

    #[test]
    fn a_permissive_existing_credential_is_refused_and_left_alone() {
        let temp = temp();
        let store = ServiceCredentialStore::new(temp.path());
        let created = store.obtain(ENV, 5, ENV_NAME).expect("create");
        let path = created.env_file();
        fs::set_permissions(path, fs::Permissions::from_mode(0o644)).expect("widen mode");
        let before = read(path);

        let error = store
            .existing(ENV, 5, ENV_NAME)
            .expect_err("a world-readable credential must be refused");
        assert!(error.to_string().contains("0600"), "got: {error}");
        assert_eq!(
            read(path),
            before,
            "a refused credential is never repaired or overwritten"
        );
        assert!(
            store.obtain(ENV, 5, ENV_NAME).is_err(),
            "obtain must not overwrite an invalid existing credential"
        );
    }

    /// Seed a credential slot with a properly private directory, so a test
    /// exercises the check it means to rather than tripping the directory
    /// guard first.
    fn seed_slot(store: &ServiceCredentialStore, generation: i64) -> std::path::PathBuf {
        let path = store.credential_path(ENV, generation).expect("path");
        let directory = path.parent().expect("parent");
        fs::create_dir_all(directory).expect("dir");
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).expect("dir mode");
        path
    }

    #[test]
    fn a_symlinked_credential_is_refused() {
        let temp = temp();
        let store = ServiceCredentialStore::new(temp.path());
        let path = seed_slot(&store, 6);
        let elsewhere = temp.path().join("elsewhere.env");
        fs::write(&elsewhere, format!("{ENV_NAME}=whatever\n")).expect("seed");
        std::os::unix::fs::symlink(&elsewhere, &path).expect("symlink");

        let error = store
            .existing(ENV, 6, ENV_NAME)
            .expect_err("a symlinked credential must be refused");
        assert!(error.to_string().contains("symlink"), "got: {error}");
        assert!(store.obtain(ENV, 6, ENV_NAME).is_err());
    }

    #[test]
    fn a_corrupt_credential_is_refused() {
        let temp = temp();
        let store = ServiceCredentialStore::new(temp.path());
        for contents in ["", "no-equals-sign\n", "DF_AGENT_TOKEN=\n", "OTHER=value\n"] {
            let path = seed_slot(&store, 7);
            fs::write(&path, contents).expect("seed");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("mode");
            assert!(
                store.existing(ENV, 7, ENV_NAME).is_err(),
                "{contents:?} must be refused"
            );
        }
    }

    #[test]
    fn the_credential_path_cannot_be_steered_by_a_request() {
        let temp = temp();
        let store = ServiceCredentialStore::new(temp.path());
        for hostile in ["../../etc/passwd", "not-a-uuid", ""] {
            assert!(
                store.credential_path(hostile, 0).is_err(),
                "{hostile:?} must not produce a path"
            );
        }
    }
}
