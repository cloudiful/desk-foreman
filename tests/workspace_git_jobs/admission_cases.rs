//! Locked admission, capacity, and derivation cases for durable Git jobs.

use chrono::{Duration as ChronoDuration, Utc};
use desk_foreman::db::workspace_git_jobs::store::{admit_and_insert_git_job, find_git_job_by_id};
use desk_foreman::db::workspace_git_jobs::types::{AdmissionError, AdmissionLimits};
use uuid::Uuid;

use super::fixtures::Fixture;
use super::support::{IssueDb, SUITE_LOCK};

async fn attempt(
    db: &IssueDb,
    _fixture: &Fixture,
    _binding: &super::fixtures::Binding,
    job: desk_foreman::db::workspace_git_jobs::types::NewGitJob,
    limits: AdmissionLimits,
) -> Result<desk_foreman::db::workspace_git_jobs::types::GitJobRecord, AdmissionError> {
    admit_and_insert_git_job(&db.pool, job, limits).await
}

#[tokio::test]
async fn admission_stores_a_secret_free_pending_reservation_with_derived_principal() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.bindings[0].clone();
    let binding_id = binding.binding_id;
    let job = fixture.new_job(&binding, "job-token", 4096);
    let limits = Fixture::usage(&db.pool, binding.binding_id, fixture.manager_id)
        .await
        .generous(4);

    let record = attempt(&db, &fixture, &binding, job.clone(), limits)
        .await
        .expect("admission succeeds");

    assert_eq!(record.state, "pending");
    assert_eq!(record.reserved_bytes, 4096);
    assert!(!record.cleanup_confirmed);
    assert!(record.claimed_at.is_none());
    assert!(record.terminal_at.is_none());
    assert_eq!(record.job_id, job.job_id);
    assert_eq!(record.token, "job-token");
    assert_eq!(
        record.principal_digest,
        fixture.principal_digest_for(&binding),
        "the principal comes from the locked binding row"
    );
    let stored = Fixture::read_state(&db.pool, job.job_id).await;
    assert_eq!(stored.principal_digest, record.principal_digest);
    assert_eq!(
        find_git_job_by_id(&db.pool, job.job_id)
            .await
            .expect("find inserted job")
            .expect("job exists")
            .state,
        "pending"
    );
    assert_eq!(binding_id, record.workspace_binding_id);

    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn invalid_inputs_are_rejected_before_any_insert() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let fixture = Fixture::setup(&db.pool).await;
    let binding = &fixture.bindings[0];
    let limits = Fixture::usage(&db.pool, binding.binding_id, fixture.manager_id)
        .await
        .generous(4);

    let mut cases = Vec::new();
    let mut invalid_token = fixture.new_job(binding, "", 1);
    cases.push(invalid_token.clone());
    invalid_token.token = " ".to_string();
    cases.push(invalid_token);
    let mut long_token = fixture.new_job(binding, &"x".repeat(129), 1);
    cases.push(long_token.clone());
    long_token.token = "valid".to_string();
    long_token.canonical_branch = "-bad".to_string();
    cases.push(long_token.clone());
    long_token.canonical_branch = "a..b".to_string();
    cases.push(long_token.clone());
    long_token.canonical_branch = "bad branch".to_string();
    cases.push(long_token.clone());
    long_token.canonical_branch = "main".to_string();
    long_token.expected_base = Some("A".repeat(40));
    cases.push(long_token.clone());
    long_token.expected_base = Some("a".repeat(41));
    cases.push(long_token.clone());
    long_token.expected_base = None;
    long_token.reserved_bytes = 0;
    cases.push(long_token.clone());
    long_token.reserved_bytes = 1;
    long_token.job_id = Uuid::nil();
    cases.push(long_token.clone());
    long_token.job_id = Uuid::new_v4();
    long_token.operation_id = Uuid::nil();
    cases.push(long_token.clone());
    long_token.operation_id = Uuid::new_v4();
    long_token.code_project_id = Uuid::nil();
    cases.push(long_token.clone());
    long_token.code_project_id = binding.code_project_id;
    long_token.authorization_generation = Uuid::nil();
    cases.push(long_token.clone());
    long_token.authorization_generation = Uuid::new_v4();
    long_token.application_id = 0;
    cases.push(long_token);

    for job in cases {
        let job_id = job.job_id;
        let outcome = attempt(&db, &fixture, binding, job, limits).await;
        assert!(
            matches!(outcome, Err(AdmissionError::InvalidInput)),
            "expected InvalidInput, got {outcome:?}"
        );
        assert!(
            find_git_job_by_id(&db.pool, job_id)
                .await
                .expect("probe invalid job")
                .is_none(),
            "an invalid admission must not insert"
        );
    }

    let invalid_limits = [
        AdmissionLimits {
            max_active: 0,
            ..limits
        },
        AdmissionLimits {
            max_bytes: 0,
            ..limits
        },
        AdmissionLimits {
            max_per_binding: -1,
            ..limits
        },
        AdmissionLimits {
            max_per_manager: 0,
            ..limits
        },
        AdmissionLimits {
            max_bytes_per_manager: 0,
            ..limits
        },
    ];
    for bad in invalid_limits {
        let job = fixture.new_job(binding, "limits-token", 1);
        let job_id = job.job_id;
        assert!(matches!(
            attempt(&db, &fixture, binding, job, bad).await,
            Err(AdmissionError::InvalidInput)
        ));
        assert!(
            find_git_job_by_id(&db.pool, job_id)
                .await
                .expect("probe limit job")
                .is_none()
        );
    }

    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn binding_resource_lease_and_deadline_mismatches_are_rejected() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.bindings[0].clone();
    let limits = Fixture::usage(&db.pool, binding.binding_id, fixture.manager_id)
        .await
        .generous(4);
    let mut rejected = Vec::new();

    let mut job = fixture.new_job(&binding, "foreign-app", 1);
    job.application_id += 1_000_000;
    rejected.push((job.token.clone(), job.job_id));
    assert!(matches!(
        attempt(&db, &fixture, &binding, job, limits).await,
        Err(AdmissionError::BindingUnavailable)
    ));

    let mut job = fixture.new_job(&binding, "foreign-resource", 1);
    job.code_project_id = Uuid::new_v4();
    rejected.push((job.token.clone(), job.job_id));
    assert!(matches!(
        attempt(&db, &fixture, &binding, job, limits).await,
        Err(AdmissionError::ResourceMismatch)
    ));

    let mut job = fixture.new_job(&binding, "foreign-owner", 1);
    job.lease_owner = "someone-else".to_string();
    rejected.push((job.token.clone(), job.job_id));
    assert!(matches!(
        attempt(&db, &fixture, &binding, job, limits).await,
        Err(AdmissionError::LeaseMismatch)
    ));

    let mut job = fixture.new_job(&binding, "foreign-epoch", 1);
    job.lease_acquired_at -= ChronoDuration::seconds(1);
    rejected.push((job.token.clone(), job.job_id));
    assert!(matches!(
        attempt(&db, &fixture, &binding, job, limits).await,
        Err(AdmissionError::LeaseMismatch)
    ));

    let past = Utc::now() - ChronoDuration::minutes(5);
    let expired = fixture
        .add_binding_with_lease(
            &db.pool,
            "code_project:expired",
            fixture.lease_owner.clone(),
            past,
            past + ChronoDuration::seconds(1),
        )
        .await;
    let mut job = fixture.new_job(&expired, "expired-lease", 1);
    job.lease_acquired_at = expired.lease_acquired_at;
    rejected.push((job.token.clone(), job.job_id));
    assert!(matches!(
        attempt(&db, &fixture, &expired, job, limits).await,
        Err(AdmissionError::LeaseExpired)
    ));

    let mut job = fixture.new_job(&binding, "past-deadline", 1);
    job.deadline_at = Utc::now() - ChronoDuration::seconds(1);
    rejected.push((job.token.clone(), job.job_id));
    assert!(matches!(
        attempt(&db, &fixture, &binding, job, limits).await,
        Err(AdmissionError::DeadlineInvalid)
    ));

    let mut job = fixture.new_job(&binding, "past-lease-deadline", 1);
    job.deadline_at = fixture.lease_expires_at + ChronoDuration::seconds(1);
    rejected.push((job.token.clone(), job.job_id));
    assert!(matches!(
        attempt(&db, &fixture, &binding, job, limits).await,
        Err(AdmissionError::DeadlineInvalid)
    ));

    for (token, job_id) in rejected {
        assert!(
            find_git_job_by_id(&db.pool, job_id)
                .await
                .expect("probe rejected admission")
                .is_none(),
            "{token} must not insert"
        );
    }

    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn a_native_task_holding_the_binding_rejects_git_admission() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let fixture = Fixture::setup(&db.pool).await;
    let binding = &fixture.bindings[0];
    fixture.seed_native_task(&db.pool, binding).await;

    let job = fixture.new_job(binding, "native-blocked", 1);
    let job_id = job.job_id;
    let limits = Fixture::usage(&db.pool, binding.binding_id, fixture.manager_id)
        .await
        .generous(4);
    assert!(matches!(
        attempt(&db, &fixture, binding, job, limits).await,
        Err(AdmissionError::NativeTaskHolds)
    ));
    assert!(
        find_git_job_by_id(&db.pool, job_id)
            .await
            .expect("probe native-blocked job")
            .is_none()
    );

    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn capacity_caps_are_all_or_nothing_and_overflow_is_typed() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let second_binding = fixture.add_binding(&db.pool, "code_project:second").await;
    let first_ref = fixture.bindings[0].clone();
    let base = Fixture::usage(&db.pool, first_ref.binding_id, fixture.manager_id).await;

    let first_job = fixture.new_job(&first_ref, "cap-first", 100);
    attempt(&db, &fixture, &first_ref, first_job, base.global_count(1))
        .await
        .expect("first reservation");

    let global_count = fixture.new_job(&second_binding, "cap-global", 100);
    assert!(matches!(
        attempt(
            &db,
            &fixture,
            &second_binding,
            global_count,
            base.global_count(1)
        )
        .await,
        Err(AdmissionError::CapacityExceeded)
    ));

    let bytes_job = fixture.new_job(&second_binding, "cap-bytes", 100);
    assert!(matches!(
        attempt(
            &db,
            &fixture,
            &second_binding,
            bytes_job,
            base.global_bytes(99)
        )
        .await,
        Err(AdmissionError::CapacityExceeded)
    ));

    let manager_bytes_job = fixture.new_job(&second_binding, "cap-manager-bytes", 100);
    assert!(matches!(
        attempt(
            &db,
            &fixture,
            &second_binding,
            manager_bytes_job,
            base.manager_bytes(99)
        )
        .await,
        Err(AdmissionError::CapacityExceeded)
    ));

    let overflow_binding = fixture.add_binding(&db.pool, "code_project:overflow").await;
    let mut overflow = fixture.new_job(&overflow_binding, "cap-overflow", i64::MAX);
    overflow.deadline_at = Utc::now() + ChronoDuration::minutes(1);
    assert!(matches!(
        attempt(&db, &fixture, &overflow_binding, overflow, base.generous(4)).await,
        Err(AdmissionError::CapacityExceeded)
    ));

    // Concurrent admission for two different bindings under one global slot:
    // exactly one reservation commits, the other is a typed capacity refusal.
    let other = fixture
        .add_binding(&db.pool, "code_project:concurrent")
        .await;
    let left = fixture.new_job(&other, "cap-left", 10);
    let right_binding = fixture
        .add_binding(&db.pool, "code_project:concurrent-2")
        .await;
    let right = fixture.new_job(&right_binding, "cap-right", 10);
    let pool = db.pool.clone();
    let limits = base.global_count(2);
    let (left, right) = tokio::join!(
        admit_and_insert_git_job(&pool, left, limits),
        admit_and_insert_git_job(&pool, right, limits),
    );
    let successes = [&left, &right]
        .iter()
        .filter(|result| result.is_ok())
        .count();
    let refusals = [&left, &right]
        .iter()
        .filter(|result| matches!(result, Err(AdmissionError::CapacityExceeded)))
        .count();
    assert_eq!(successes, 1, "exactly one concurrent reservation commits");
    assert_eq!(refusals, 1, "the loser is a typed capacity refusal");

    fixture.cleanup(&db.pool).await;
}
