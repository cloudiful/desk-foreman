//! Unit tests for durable execution environment helpers (no live DB).
//!
//! Covers the pure invariants: state allowlist, volume-ref derivation,
//! and opaque-input validation. Database concurrency and tenant fencing
//! are covered by `tests/execution_environments.rs` against an isolated
//! nonproduction PostgreSQL.

use uuid::Uuid;

use super::types::{
    EXECUTION_ENVIRONMENT_INITIAL_STATE, EXECUTION_ENVIRONMENT_INVALIDATED_STATE,
    derive_state_volume_ref, execution_state_is_valid, external_user_id_is_valid,
    runtime_kind_is_valid,
};

#[test]
fn lifecycle_states_match_checked_constraint() {
    for state in [
        "absent",
        "starting",
        "ready",
        "stopping",
        "replaced",
        "unreachable",
    ] {
        assert!(
            execution_state_is_valid(state),
            "state {state} must be valid"
        );
    }
    for state in ["", "running", "STARTING", "deleted", "stopped"] {
        assert!(
            !execution_state_is_valid(state),
            "state {state} must be rejected"
        );
    }
}

#[test]
fn initial_and_invalidated_states_are_checked_values() {
    assert_eq!(EXECUTION_ENVIRONMENT_INITIAL_STATE, "starting");
    assert_eq!(EXECUTION_ENVIRONMENT_INVALIDATED_STATE, "stopping");
    assert!(execution_state_is_valid(
        EXECUTION_ENVIRONMENT_INITIAL_STATE
    ));
    assert!(execution_state_is_valid(
        EXECUTION_ENVIRONMENT_INVALIDATED_STATE
    ));
}

#[test]
fn state_volume_ref_derives_from_uuid_simple() {
    let id = Uuid::parse_str("123e4567-e89b-12d3-a456-426614174000").unwrap();
    assert_eq!(
        derive_state_volume_ref(&id),
        "df-env-123e4567e89b12d3a456426614174000"
    );
    let fresh = Uuid::new_v4();
    let derived = derive_state_volume_ref(&fresh);
    assert!(derived.starts_with("df-env-"));
    assert_eq!(derived.len(), "df-env-".len() + 32);
    let suffix = derived.strip_prefix("df-env-").expect("prefix");
    assert!(!suffix.contains('-'));
}

#[test]
fn opaque_inputs_reject_blank_and_oversize() {
    assert!(!runtime_kind_is_valid(""));
    assert!(!runtime_kind_is_valid("   "));
    assert!(runtime_kind_is_valid("opencode"));
    assert!(runtime_kind_is_valid(" custom-kind_1 "));
    assert!(!runtime_kind_is_valid(&"x".repeat(65)));

    assert!(!external_user_id_is_valid(""));
    assert!(!external_user_id_is_valid("  "));
    assert!(external_user_id_is_valid("user-123"));
    assert!(!external_user_id_is_valid(&"x".repeat(129)));
}
