//! Pure unit tests for the durable Git-job foundation. Database behavior is
//! covered by the guarded `workspace_git_jobs` integration suite.

use super::types::{
    AdmissionError, AdmissionLimits, GIT_JOB_ACTIVE_STATES, GitJobCompletion,
    git_job_state_is_active, git_job_state_is_terminal, valid_expected_base, valid_token,
};

#[test]
fn active_and_terminal_state_sets_are_disjoint() {
    for state in GIT_JOB_ACTIVE_STATES {
        assert!(git_job_state_is_active(state));
        assert!(!git_job_state_is_terminal(state));
    }
    assert!(git_job_state_is_terminal("succeeded"));
    assert!(git_job_state_is_terminal("failed"));
    assert!(git_job_state_is_terminal("superseded"));
    assert!(!git_job_state_is_active("succeeded"));
}

#[test]
fn admission_limits_reject_zero_and_negative_values() {
    let valid = AdmissionLimits {
        max_active: 1,
        max_bytes: 1,
        max_per_binding: 1,
        max_per_manager: 1,
        max_bytes_per_manager: 1,
    };
    assert!(valid.validate().is_ok());
    for invalid in [
        AdmissionLimits {
            max_active: 0,
            ..valid
        },
        AdmissionLimits {
            max_bytes: 0,
            ..valid
        },
        AdmissionLimits {
            max_per_binding: -1,
            ..valid
        },
        AdmissionLimits {
            max_per_manager: 0,
            ..valid
        },
        AdmissionLimits {
            max_bytes_per_manager: 0,
            ..valid
        },
    ] {
        assert!(matches!(
            invalid.validate(),
            Err(AdmissionError::InvalidInput)
        ));
    }
}

#[test]
fn tokens_and_expected_bases_are_bounded_before_queries() {
    assert!(valid_token("job-token"));
    assert!(!valid_token(" "));
    assert!(!valid_token(&"x".repeat(129)));
    assert!(valid_expected_base(None));
    assert!(valid_expected_base(Some(&"a".repeat(40))));
    assert!(valid_expected_base(Some(&"b".repeat(64))));
    assert!(!valid_expected_base(Some(&"c".repeat(41))));
}

#[test]
fn completion_uncertainty_retains_active_state() {
    let uncertain = GitJobCompletion::Failed {
        error_summary: "cleanup not confirmed".to_owned(),
        cleanup_confirmed: false,
    };
    assert_eq!(uncertain.state(), "uncertain");
    assert!(!uncertain.cleanup_confirmed());
    assert_eq!(uncertain.result_value().expect("result"), None);
    assert!(uncertain.error_summary().is_some());
}
