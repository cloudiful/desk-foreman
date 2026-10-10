//! Durable barrier store unit tests (issue 742 P1, `#[cfg(test)]`).
//!
//! Proves monotonic high-water, durable reopen, and fail-closed handling of
//! corrupt, invalid, unsupported, or oversize state on the real filesystem.
//! No Docker and no live daemon.

#[cfg(test)]
mod tombstone_tests {
    use std::path::Path;

    use super::super::test_support as support;
    use super::super::tombstone::{
        EnvironmentBarrier, TombstoneStore, advance_high_water, ensure_generation_admissible,
        record_stopped,
    };

    fn store(state_root: &Path) -> TombstoneStore {
        TombstoneStore::open(state_root, support::ENV_ID).expect("open store")
    }

    #[test]
    fn missing_barrier_is_empty_and_persists_monotonically() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        let first = store(&state);
        assert_eq!(first.load().expect("load"), EnvironmentBarrier::default());

        first
            .save(advance_high_water(first.load().unwrap(), 3))
            .expect("save attempt 3");
        drop(first);

        // Reopen: the barrier survives a fresh store instance.
        let reopened = store(&state);
        assert_eq!(
            reopened.load().expect("reload"),
            EnvironmentBarrier {
                high_water: 3,
                stopped_generation: None,
            }
        );

        // A lower attempt never rewinds the high-water.
        reopened
            .save(advance_high_water(reopened.load().unwrap(), 1))
            .expect("save attempt 1");
        assert_eq!(reopened.load().unwrap().high_water, 3);

        // A stop records both high-water and the stopped generation.
        reopened
            .save(record_stopped(reopened.load().unwrap(), 5))
            .expect("save stop 5");
        let barrier = reopened.load().unwrap();
        assert_eq!(barrier.high_water, 5);
        assert_eq!(barrier.stopped_generation, Some(5));
    }

    #[test]
    fn admissibility_refuses_closed_and_lower_generations() {
        let barrier = EnvironmentBarrier {
            high_water: 4,
            stopped_generation: Some(2),
        };
        assert!(ensure_generation_admissible(&barrier, 4).is_ok());
        assert!(ensure_generation_admissible(&barrier, 5).is_ok());
        assert!(ensure_generation_admissible(&barrier, 2).is_err());
        assert!(ensure_generation_admissible(&barrier, 1).is_err());

        let high_only = EnvironmentBarrier {
            high_water: 4,
            stopped_generation: None,
        };
        assert!(ensure_generation_admissible(&high_only, 3).is_err());
    }

    fn write_barrier_file(state_root: &Path, contents: &[u8]) {
        let path = support::barrier_path(state_root);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("state dir");
        std::fs::write(&path, contents).expect("barrier file");
    }

    #[test]
    fn corrupt_invalid_unsupported_and_oversize_state_fail_closed() {
        for contents in [
            b"not-json".as_slice(),
            br#"{"version":1,"high_water":-1}"#.as_slice(),
            br#"{"version":1,"high_water":1,"stopped_generation":5}"#.as_slice(),
            br#"{"version":2,"high_water":1}"#.as_slice(),
            br#"{"version":1,"high_water":1,"unknown":true}"#.as_slice(),
        ] {
            let temp = tempfile::tempdir().expect("tempdir");
            let state = support::state_dir(temp.path());
            write_barrier_file(&state, contents);
            let error = store(&state).load().expect_err("invalid state must fail");
            assert!(!error.to_string().is_empty());
        }

        let temp = tempfile::tempdir().expect("tempdir");
        let state = support::state_dir(temp.path());
        write_barrier_file(&state, &vec![b' '; 8192]);
        assert!(store(&state).load().is_err());
    }
}
