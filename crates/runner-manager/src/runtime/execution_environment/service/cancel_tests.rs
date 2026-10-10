//! Cancellation-registry tests (issue 749 P2c, `#[cfg(test)]`).
//!
//! No Docker, no network: what these prove is that a stop wakes exactly the
//! streams it should, that a woken stream observes the wake even before its
//! first poll, and that a repeated cancel wakes nothing twice.

#[cfg(test)]
mod cancel_tests {
    use super::super::cancel::ServiceCancelRegistry;

    #[test]
    fn a_cancel_wakes_only_its_own_environment() {
        let registry = ServiceCancelRegistry::new();
        let stopped = registry.register("env-stopped");
        let kept = registry.register("env-kept");
        assert_eq!(registry.cancel_environment("env-stopped"), 1);
        assert!(
            *stopped.borrow(),
            "a cancelled stream must observe the wake"
        );
        assert!(
            !*kept.borrow(),
            "another environment keeps serving after a foreign stop"
        );
    }

    #[test]
    fn a_repeated_cancel_wakes_nothing_twice() {
        let registry = ServiceCancelRegistry::new();
        let _stream = registry.register("env");
        assert_eq!(registry.cancel_environment("env"), 1);
        assert_eq!(
            registry.cancel_environment("env"),
            0,
            "the first cancel removed the entry"
        );
    }

    #[test]
    fn a_cancel_with_no_live_stream_wakes_nothing() {
        let registry = ServiceCancelRegistry::new();
        assert_eq!(registry.cancel_environment("env-absent"), 0);
    }

    #[test]
    fn a_dropped_stream_is_not_counted_again() {
        let registry = ServiceCancelRegistry::new();
        let first = registry.register("env");
        let _second = registry.register("env");
        drop(first);
        assert_eq!(
            registry.cancel_environment("env"),
            1,
            "a gone receiver is pruned at registration, not woken"
        );
    }

    #[tokio::test]
    async fn a_cancel_before_the_first_poll_still_fires() {
        let registry = ServiceCancelRegistry::new();
        let mut receiver = registry.register("env");
        registry.cancel_environment("env");
        receiver
            .wait_for(|cancelled| *cancelled)
            .await
            .expect("a sticky wake survives until the first poll");
    }
}
