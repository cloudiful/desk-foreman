//! Host-identity and filesystem-guard tests (issue 749 P2a repair,
//! `#[cfg(test)]`).
//!
//! Covers the effective-uid reader (including its failure modes), the
//! directory-chain rules, and the handle-based credential validation. No
//! privileged operations: ownership differences are simulated through the
//! injectable uid source, not by chowning files to another account.

#[cfg(test)]
mod guard_tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use super::super::credential::ServiceCredentialStore;
    use super::super::fs_guard::{
        CREDENTIAL_DIR_MODE, CREDENTIAL_MODE, MAX_CREDENTIAL_BYTES, assert_private_directory,
        prepare_private_directory, read_credential_from_handle, refuse_link_or_irregular,
        validate_directory_chain,
    };
    use super::super::host_identity::{UidSource, parse_effective_uid, read_effective_uid};
    use super::super::profile::{ServiceProfileEnv, parse_service_profile};

    const ENV: &str = "123e4567-e89b-12d3-a456-426614174000";
    const ENV_NAME: &str = "DF_AGENT_TOKEN";

    fn temp() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn store(temp: &tempfile::TempDir) -> ServiceCredentialStore {
        ServiceCredentialStore::new(temp.path())
    }

    fn real_uid() -> u32 {
        UidSource::Proc
            .effective_uid()
            .expect("procfs is available on this host")
    }

    fn mode_of(path: &Path) -> u32 {
        fs::symlink_metadata(path)
            .expect("stat")
            .permissions()
            .mode()
            & 0o777
    }

    // ---- effective uid reader ----

    #[test]
    fn effective_uid_is_the_second_field_not_the_real_uid() {
        let status = "Name:\tbash\nUid:\t1000\t1001\t1002\t1000\nGid:\t100\t100\t100\t100\n";
        assert_eq!(
            parse_effective_uid(status).expect("parsed"),
            1001,
            "ownership is judged by the effective uid, not the real one"
        );
    }

    #[test]
    fn effective_uid_reader_fails_closed_when_unavailable() {
        for broken in [
            "",
            "Name:\tbash\n",
            "Uid:\n",
            "Uid:\t1000\n",
            "Uid:\t1000\tnot-a-number\n",
            "Uid:\t\t1001\n",
        ] {
            assert!(
                parse_effective_uid(broken).is_err(),
                "{broken:?} must fail closed rather than skip the owner check"
            );
        }
        assert!(read_effective_uid("/nonexistent/proc/self/status").is_err());
        assert!(UidSource::Unavailable.effective_uid().is_err());
    }

    #[tokio::test]
    async fn an_unresolvable_owner_refuses_the_credential_instead_of_skipping_it() {
        let temp = temp();
        let blind = ServiceCredentialStore::with_uid_source(temp.path(), UidSource::Unavailable);
        let error = blind
            .obtain(ENV, 0, ENV_NAME)
            .expect_err("ownership that cannot be established must not be treated as correct");
        assert!(
            error.to_string().contains("effective uid is unavailable"),
            "got: {error}"
        );
        assert!(
            !blind.credential_path(ENV, 0).expect("path").exists(),
            "nothing is written when the owner cannot be established"
        );
    }

    #[tokio::test]
    async fn a_credential_owned_by_another_uid_is_refused() {
        let temp = temp();
        let owner = store(&temp);
        owner.obtain(ENV, 0, ENV_NAME).expect("create");
        let impostor = ServiceCredentialStore::with_uid_source(
            temp.path(),
            UidSource::Fixed(real_uid().wrapping_add(1)),
        );
        assert!(
            impostor.existing(ENV, 0, ENV_NAME).is_err(),
            "a file owned by another user must not be accepted"
        );
    }

    // ---- directory chain ----

    #[test]
    fn a_prepared_directory_is_owner_only() {
        let temp = temp();
        let directory = temp.path().join("env").join("service");
        prepare_private_directory(temp.path(), &directory).expect("prepare");
        assert_eq!(mode_of(&directory), CREDENTIAL_DIR_MODE);
        assert!(assert_private_directory(&directory, real_uid()).is_ok());
    }

    #[test]
    fn a_permissive_reused_directory_is_refused_and_left_alone() {
        let temp = temp();
        let directory = temp.path().join("env").join("service");
        prepare_private_directory(temp.path(), &directory).expect("prepare");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).expect("widen");
        let before = mode_of(&directory);

        let error = assert_private_directory(&directory, real_uid())
            .expect_err("a world-accessible directory must be refused");
        assert!(error.to_string().contains("accessible"), "got: {error}");
        assert_eq!(
            mode_of(&directory),
            before,
            "a refused directory is never silently repaired"
        );
        assert!(
            validate_directory_chain(temp.path(), &directory).is_ok(),
            "the chain itself is still real directories"
        );
    }

    #[test]
    fn a_symlinked_directory_in_the_chain_is_refused() {
        let temp = temp();
        let real = temp.path().join("real");
        fs::create_dir_all(&real).expect("dir");
        let linked = temp.path().join("linked");
        std::os::unix::fs::symlink(&real, &linked).expect("symlink");

        let directory = linked.join("service");
        let error = validate_directory_chain(temp.path(), &directory)
            .expect_err("a symlinked directory component must be refused");
        assert!(error.to_string().contains("symlink"), "got: {error}");
        assert!(prepare_private_directory(temp.path(), &directory).is_err());
    }

    #[tokio::test]
    async fn a_store_whose_service_directory_is_permissive_refuses_to_read() {
        let temp = temp();
        let owner = store(&temp);
        owner.obtain(ENV, 1, ENV_NAME).expect("create");
        let directory = owner
            .credential_path(ENV, 1)
            .expect("path")
            .parent()
            .expect("parent")
            .to_path_buf();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).expect("widen");

        let error = owner
            .existing(ENV, 1, ENV_NAME)
            .expect_err("a permissive directory must block the read path too");
        assert!(error.to_string().contains("accessible"), "got: {error}");
    }

    // ---- handle-based validation ----

    #[test]
    fn a_credential_is_validated_from_the_open_handle_not_the_path() {
        let temp = temp();
        let path = temp.path().join("7.env");
        fs::write(&path, format!("{ENV_NAME}=abc\n")).expect("seed");
        fs::set_permissions(&path, fs::Permissions::from_mode(CREDENTIAL_MODE)).expect("mode");

        let file = fs::File::open(&path).expect("open");
        // Swap the path out from under the caller after it was opened; the
        // handle still refers to the original, validated file.
        fs::rename(&path, temp.path().join("moved.env")).expect("move");
        fs::write(&path, format!("{ENV_NAME}=attacker\n")).expect("replace");
        fs::set_permissions(&path, fs::Permissions::from_mode(CREDENTIAL_MODE)).expect("mode");

        assert!(
            read_credential_from_handle(&file, ENV_NAME, real_uid()).is_ok(),
            "content comes from the handle, so a post-open path swap cannot change it"
        );
    }

    #[test]
    fn a_handle_pointing_at_a_wrong_file_is_refused() {
        let temp = temp();
        let path = temp.path().join("8.env");
        let uid = real_uid();
        let cases: [(&str, u32, &str); 3] = [
            (&format!("{ENV_NAME}=abc\n"), 0o644, "permissive"),
            (&format!("{ENV_NAME}=abc\n"), CREDENTIAL_MODE, "right"),
            (&"OTHER=abc\n", CREDENTIAL_MODE, "wrong name"),
        ];
        for (contents, mode, _label) in cases {
            fs::write(&path, contents).expect("seed");
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("mode");
            let file = fs::File::open(&path).expect("open");
            let ok = read_credential_from_handle(&file, ENV_NAME, uid).is_ok();
            assert_eq!(
                ok,
                mode == CREDENTIAL_MODE && contents.starts_with(ENV_NAME)
            );
        }
    }

    #[test]
    fn an_oversized_credential_is_refused_from_the_handle() {
        let temp = temp();
        let path = temp.path().join("9.env");
        let huge = format!("{ENV_NAME}={}\n", "a".repeat(MAX_CREDENTIAL_BYTES as usize));
        fs::write(&path, &huge).expect("seed");
        fs::set_permissions(&path, fs::Permissions::from_mode(CREDENTIAL_MODE)).expect("mode");
        let file = fs::File::open(&path).expect("open");
        assert!(
            read_credential_from_handle(&file, ENV_NAME, real_uid()).is_err(),
            "an oversized credential is refused"
        );
    }

    #[test]
    fn a_symlinked_credential_is_refused_before_it_is_opened() {
        let temp = temp();
        let target = temp.path().join("real.env");
        let link = temp.path().join("link.env");
        fs::write(&target, format!("{ENV_NAME}=abc\n")).expect("seed");
        fs::set_permissions(&target, fs::Permissions::from_mode(CREDENTIAL_MODE)).expect("mode");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        let error = refuse_link_or_irregular(&link).expect_err("a symlink must be refused");
        assert!(error.to_string().contains("symlink"), "got: {error}");
    }

    #[test]
    fn a_directory_in_place_of_a_credential_is_refused() {
        let temp = temp();
        let path = temp.path().join("notafile");
        fs::create_dir_all(&path).expect("dir");
        assert!(refuse_link_or_irregular(&path).is_err());
    }

    #[test]
    fn the_absent_service_profile_still_needs_nothing_from_the_store() {
        assert!(
            parse_service_profile(ServiceProfileEnv::default())
                .eq(&super::super::profile::ServiceProfile::Unset)
        );
    }
}
