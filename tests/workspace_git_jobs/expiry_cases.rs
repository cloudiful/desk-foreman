//! Expiry and closed completion-replay cases for durable Git jobs.

use chrono::{Duration as ChronoDuration, Utc};
use desk_foreman::db::workspace_git_jobs::lifecycle::{
    CompleteOutcome, complete_git_job, expire_git_jobs,
};
use desk_foreman::db::workspace_git_jobs::store::find_git_job_by_id;
use desk_foreman::db::workspace_git_jobs::types::{GitJobCompletion, GitJobResult};
use uuid::Uuid;

use super::fixtures::{Fixture, ManagerHarness, SeedJob, insert_credential, synthetic_credential};
use super::support::{IssueDb, SUITE_LOCK};

const INSERT_GIT_JOB: &str = include_str!("fixtures/insert_git_job.sql");

fn head() -> String {
    "a".repeat(40)
}

fn succeeded() -> GitJobCompletion {
    GitJobCompletion::Succeeded {
        result: GitJobResult::SyncSucceeded {
            head: head(),
            base: None,
            published: true,
        },
    }
}

#[tokio::test]
async fn pending_expiry_releases_only_never_claimed_reservations() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let pending_binding = fixture
        .add_binding(&db.pool, "code_project:expiry-pending")
        .await;
    let claimed_binding = fixture
        .add_binding(&db.pool, "code_project:expiry-claimed")
        .await;
    let uncertain_binding = fixture
        .add_binding(&db.pool, "code_project:expiry-uncertain")
        .await;
    let past = Utc::now() - ChronoDuration::minutes(1);
    let pending = fixture
        .seed_git_job(
            &db.pool,
            SeedJob {
                binding: &pending_binding,
                state: "pending",
                reserved_bytes: 64,
                deadline_at: past,
                claimed_at: None,
                cleanup_confirmed: false,
                terminal_at: None,
            },
        )
        .await;
    let claimed = fixture
        .seed_git_job(
            &db.pool,
            SeedJob {
                binding: &claimed_binding,
                state: "pending",
                reserved_bytes: 64,
                deadline_at: past,
                claimed_at: None,
                cleanup_confirmed: false,
                terminal_at: None,
            },
        )
        .await;
    Fixture::mark_claimed(&db.pool, claimed).await;
    Fixture::set_deadline(&db.pool, claimed, past).await;
    let uncertain = fixture
        .seed_git_job(
            &db.pool,
            SeedJob {
                binding: &uncertain_binding,
                state: "uncertain",
                reserved_bytes: 64,
                deadline_at: past,
                claimed_at: Some(Utc::now() - ChronoDuration::minutes(3)),
                cleanup_confirmed: false,
                terminal_at: None,
            },
        )
        .await;

    let expired = expire_git_jobs(&db.pool).await.expect("expire jobs");
    let expired_ids: Vec<Uuid> = expired.iter().map(|row| row.job_id).collect();
    assert!(expired_ids.contains(&pending));
    assert!(expired_ids.contains(&claimed));
    assert!(!expired_ids.contains(&uncertain));

    let pending_state = Fixture::read_state(&db.pool, pending).await;
    assert_eq!(pending_state.state, "failed");
    assert!(pending_state.cleanup_confirmed);
    assert!(pending_state.claimed_at.is_none());
    assert!(pending_state.terminal_at.is_some());

    let claimed_state = Fixture::read_state(&db.pool, claimed).await;
    assert_eq!(claimed_state.state, "uncertain");
    assert!(!claimed_state.cleanup_confirmed);
    assert_eq!(claimed_state.reserved_bytes, 64);
    assert!(claimed_state.claimed_at.is_some());
    assert!(
        claimed_state.terminal_at.is_none(),
        "a potentially started job keeps no terminal timestamp"
    );

    let uncertain_state = Fixture::read_state(&db.pool, uncertain).await;
    assert_eq!(uncertain_state.state, "uncertain");
    assert!(!uncertain_state.cleanup_confirmed);
    assert!(uncertain_state.terminal_at.is_none());

    // A second sweep never moves the retained uncertain reservation.
    let second = expire_git_jobs(&db.pool).await.expect("second expiry");
    assert!(second.is_empty());
    assert_eq!(
        Fixture::read_state(&db.pool, uncertain).await.state,
        "uncertain"
    );

    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn completion_requires_matching_token_manager_state_and_deadline() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.bindings[0].clone();
    let token = "completion-token";
    let job = fixture
        .seed_git_job(
            &db.pool,
            SeedJob {
                binding: &binding,
                state: "claimed",
                reserved_bytes: 8,
                deadline_at: Utc::now() + ChronoDuration::minutes(5),
                claimed_at: Some(Utc::now()),
                cleanup_confirmed: false,
                terminal_at: None,
            },
        )
        .await;
    let seeded = seeded_token(&db.pool, job).await;

    assert!(matches!(
        complete_git_job(&db.pool, job, token, fixture.manager_id, succeeded())
            .await
            .expect("wrong token completion"),
        CompleteOutcome::NotCompletable { current: None }
    ));
    assert!(matches!(
        complete_git_job(&db.pool, job, &seeded, fixture.manager_id + 1, succeeded())
            .await
            .expect("wrong manager completion"),
        CompleteOutcome::NotCompletable { current: None }
    ));
    assert_eq!(Fixture::read_state(&db.pool, job).await.state, "claimed");

    let pending_binding = fixture
        .add_binding(&db.pool, "code_project:completion-pending")
        .await;
    let pending = fixture
        .seed_git_job(
            &db.pool,
            SeedJob {
                binding: &pending_binding,
                state: "pending",
                reserved_bytes: 8,
                deadline_at: Utc::now() + ChronoDuration::minutes(5),
                claimed_at: None,
                cleanup_confirmed: false,
                terminal_at: None,
            },
        )
        .await;
    assert!(matches!(
        complete_git_job(
            &db.pool,
            pending,
            &seeded_token(&db.pool, pending).await,
            fixture.manager_id,
            succeeded()
        )
        .await
        .expect("pending completion"),
        CompleteOutcome::NotCompletable { current: Some(_) }
    ));

    Fixture::set_deadline(&db.pool, job, Utc::now() - ChronoDuration::seconds(1)).await;
    assert!(matches!(
        complete_git_job(&db.pool, job, &seeded, fixture.manager_id, succeeded())
            .await
            .expect("expired completion"),
        CompleteOutcome::NotCompletable { current: Some(_) }
    ));
    assert_eq!(Fixture::read_state(&db.pool, job).await.state, "claimed");

    fixture.cleanup(&db.pool).await;
}

async fn seeded_token(pool: &sqlx::PgPool, job: Uuid) -> String {
    Fixture::read_state(pool, job).await.token
}

#[tokio::test]
async fn terminal_replay_is_exact_and_contradictions_do_not_mutate() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.bindings[0].clone();
    let job = fixture
        .seed_git_job(
            &db.pool,
            SeedJob {
                binding: &binding,
                state: "claimed",
                reserved_bytes: 16,
                deadline_at: Utc::now() + ChronoDuration::minutes(5),
                claimed_at: Some(Utc::now()),
                cleanup_confirmed: false,
                terminal_at: None,
            },
        )
        .await;
    let token = seeded_token(&db.pool, job).await;

    let completed = complete_git_job(&db.pool, job, &token, fixture.manager_id, succeeded())
        .await
        .expect("complete job");
    assert!(matches!(completed, CompleteOutcome::Completed(_)));
    let state = Fixture::read_state(&db.pool, job).await;
    assert_eq!(state.state, "succeeded");
    assert!(state.cleanup_confirmed);
    assert!(state.terminal_at.is_some());

    let replay = complete_git_job(&db.pool, job, &token, fixture.manager_id, succeeded())
        .await
        .expect("replay");
    assert!(matches!(replay, CompleteOutcome::AlreadyTerminal(_)));

    let contradiction = complete_git_job(
        &db.pool,
        job,
        &token,
        fixture.manager_id,
        GitJobCompletion::Failed {
            error_summary: "different outcome".to_string(),
            cleanup_confirmed: true,
        },
    )
    .await
    .expect("contradiction");
    assert!(matches!(
        contradiction,
        CompleteOutcome::TerminalConflict { .. }
    ));
    assert_eq!(Fixture::read_state(&db.pool, job).await.state, "succeeded");

    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn unconfirmed_cleanup_stays_uncertain_and_terminal_shapes_are_constrained() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.bindings[0].clone();
    let job = fixture
        .seed_git_job(
            &db.pool,
            SeedJob {
                binding: &binding,
                state: "claimed",
                reserved_bytes: 32,
                deadline_at: Utc::now() + ChronoDuration::minutes(5),
                claimed_at: Some(Utc::now()),
                cleanup_confirmed: false,
                terminal_at: None,
            },
        )
        .await;
    let token = seeded_token(&db.pool, job).await;

    let outcome = complete_git_job(
        &db.pool,
        job,
        &token,
        fixture.manager_id,
        GitJobCompletion::Failed {
            error_summary: "cleanup unknown".to_string(),
            cleanup_confirmed: false,
        },
    )
    .await
    .expect("uncertain completion");
    assert!(matches!(outcome, CompleteOutcome::Completed(_)));
    let state = Fixture::read_state(&db.pool, job).await;
    assert_eq!(state.state, "uncertain");
    assert!(!state.cleanup_confirmed);
    assert!(state.terminal_at.is_none());

    for (state, cleanup_confirmed, terminal_at) in [
        ("succeeded", false, None::<chrono::DateTime<Utc>>),
        ("failed", false, None),
        ("succeeded", true, None),
    ] {
        let result = sqlx::query(INSERT_GIT_JOB)
            .bind(Uuid::new_v4())
            .bind(format!("invalid-{}", Uuid::new_v4().simple()))
            .bind(Uuid::new_v4())
            .bind(fixture.application_id)
            .bind(binding.binding_id)
            .bind(fixture.manager_id)
            .bind(binding.code_project_id)
            .bind(Uuid::new_v4())
            .bind(state)
            .bind("main")
            .bind(Option::<String>::None)
            .bind(fixture.principal_digest_for(&binding))
            .bind(&fixture.lease_owner)
            .bind(fixture.lease_acquired_at)
            .bind(1_i64)
            .bind(Some(Utc::now()))
            .bind(Utc::now() + ChronoDuration::minutes(1))
            .bind(cleanup_confirmed)
            .bind(terminal_at)
            .execute(&db.pool)
            .await;
        assert!(
            result.is_err(),
            "terminal shape {state}/{cleanup_confirmed} must be rejected"
        );
    }

    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn production_git_expiry_driver_ticks_and_prunes_credentials() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let pending_binding = fixture
        .add_binding(&db.pool, "code_project:janitor-pending")
        .await;
    let claimed_binding = fixture
        .add_binding(&db.pool, "code_project:janitor-claimed")
        .await;
    let past = Utc::now() - ChronoDuration::minutes(1);
    let pending = fixture
        .seed_git_job(
            &db.pool,
            SeedJob {
                binding: &pending_binding,
                state: "pending",
                reserved_bytes: 64,
                deadline_at: past,
                claimed_at: None,
                cleanup_confirmed: false,
                terminal_at: None,
            },
        )
        .await;
    let claimed = fixture
        .seed_git_job(
            &db.pool,
            SeedJob {
                binding: &claimed_binding,
                state: "pending",
                reserved_bytes: 64,
                deadline_at: past,
                claimed_at: None,
                cleanup_confirmed: false,
                terminal_at: None,
            },
        )
        .await;
    Fixture::mark_claimed(&db.pool, claimed).await;
    Fixture::set_deadline(&db.pool, claimed, past).await;

    let harness = ManagerHarness::setup(db.pool.clone()).await;
    let state = harness.state().clone();
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let driver = tokio::spawn(desk_foreman::lifecycle::run_git_job_expiry(
        state.clone(),
        shutdown_rx,
        std::time::Duration::from_millis(50),
    ));

    let mut pending_state = Fixture::read_state(&db.pool, pending).await;
    let mut claimed_state = Fixture::read_state(&db.pool, claimed).await;
    for _ in 0..100 {
        if pending_state.state == "failed" && claimed_state.state == "uncertain" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        pending_state = Fixture::read_state(&db.pool, pending).await;
        claimed_state = Fixture::read_state(&db.pool, claimed).await;
    }
    assert_eq!(pending_state.state, "failed");
    assert!(pending_state.cleanup_confirmed);
    assert!(pending_state.terminal_at.is_some());
    assert_eq!(claimed_state.state, "uncertain");
    assert!(!claimed_state.cleanup_confirmed);
    assert!(claimed_state.terminal_at.is_none());

    let row = find_git_job_by_id(&db.pool, pending)
        .await
        .expect("find expired row")
        .expect("expired row");
    insert_credential(
        &state,
        &row,
        synthetic_credential(row.code_project_id, row.deadline_at.timestamp_millis(), 'a'),
    );

    let cadence_binding = fixture
        .add_binding(&db.pool, "code_project:janitor-cadence")
        .await;
    let cadence = fixture
        .seed_git_job(
            &db.pool,
            SeedJob {
                binding: &cadence_binding,
                state: "pending",
                reserved_bytes: 64,
                deadline_at: past,
                claimed_at: None,
                cleanup_confirmed: false,
                terminal_at: None,
            },
        )
        .await;
    let mut cadence_state = Fixture::read_state(&db.pool, cadence).await;
    for _ in 0..100 {
        if cadence_state.state == "failed" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        cadence_state = Fixture::read_state(&db.pool, cadence).await;
    }
    assert_eq!(cadence_state.state, "failed");

    shutdown_tx.send(true).expect("shutdown");
    let report = driver.await.expect("join driver");
    assert!(report.expired_jobs >= 3, "{report:?}");
    assert!(report.pruned_credentials >= 1, "{report:?}");
    fixture.cleanup(&db.pool).await;
}
