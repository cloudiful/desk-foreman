//! Filesystem reservation unit tests (issue 742 P1, `#[cfg(test)]`).
//!
//! Proves `create_new` exclusivity, holder-token-protected release, and
//! fail-closed behavior on existing or crash-held reservations. No Docker
//! and no live daemon.

#[cfg(test)]
mod file_lock_tests {
    use std::path::Path;

    use super::super::file_lock::{EnvFileLock, lock_holder_token};

    fn lock_path(temp: &Path) -> std::path::PathBuf {
        temp.join("execution_environment.lock")
    }

    #[test]
    fn existing_reservation_rejects_a_second_holder() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = lock_path(temp.path());

        let first = EnvFileLock::acquire(&path, lock_holder_token("manager-a")).expect("first");
        let error = EnvFileLock::acquire(&path, lock_holder_token("manager-b"))
            .expect_err("second holder must be rejected");
        assert!(error.to_string().contains("already held"));
        assert!(path.exists(), "a rejected acquire must not remove the lock");

        first.release().expect("release own lock");
        assert!(!path.exists());

        // The path is free again after an ordinary release.
        EnvFileLock::acquire(&path, lock_holder_token("manager-b")).expect("reacquire");
    }

    #[test]
    fn crash_held_reservation_blocks_without_stale_cleanup() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = lock_path(temp.path());
        std::fs::write(&path, "manager=dead;pid=999999;nonce=1").expect("seed crash lock");

        let error = EnvFileLock::acquire(&path, lock_holder_token("manager-a"))
            .expect_err("crash-held lock must fail closed");
        assert!(error.to_string().contains("operator recovery"));
        assert!(path.exists(), "crash-held lock must never be auto-removed");
    }

    #[test]
    fn release_refuses_a_foreign_holder_and_keeps_the_reservation() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = lock_path(temp.path());
        let guard =
            EnvFileLock::acquire(&path, "manager=ours;pid=1;nonce=1".to_string()).expect("acquire");

        // Simulate another identity taking over the path.
        std::fs::write(&path, "manager=foreign;pid=2;nonce=2").expect("overwrite token");
        let error = guard
            .release()
            .expect_err("foreign holder must not be removed");
        assert!(error.to_string().contains("another identity"));
        assert!(path.exists());
    }

    #[test]
    fn holder_tokens_are_distinct() {
        assert_ne!(lock_holder_token("m"), lock_holder_token("m"));
        assert!(lock_holder_token("m").contains("manager=m"));
    }
}
