//! Claim and secret-free wire cases for the Git job control plane.

use chrono::{Duration as ChronoDuration, Utc};
use desk_foreman::db::workspace_git_jobs::lifecycle::claim_next_git_job;
use desk_foreman::db::workspace_git_jobs::store::find_git_job_by_id;
use runner_protocol::{GitJobSpec, RunnerJob};
use uuid::Uuid;

use super::fixtures::{Binding, Fixture, ManagerHarness, SeedJob};
use super::support::{IssueDb, SUITE_LOCK};

async fn seed(db: &IssueDb, fixture: &Fixture, binding: &Binding, state: &str) -> Uuid {
    fixture
        .seed_git_job(
            &db.pool,
            SeedJob {
                binding,
                state,
                reserved_bytes: 128,
                deadline_at: Utc::now() + ChronoDuration::minutes(5),
                claimed_at: (state != "pending").then(Utc::now),
                cleanup_confirmed: false,
                terminal_at: None,
            },
        )
        .await
}

#[tokio::test]
async fn claim_takes_only_the_oldest_pending_job_for_the_assigned_manager() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.bindings[0].clone();

    let first = seed(&db, &fixture, &binding, "pending").await;
    let claimed = claim_next_git_job(&db.pool, fixture.manager_id)
        .await
        .expect("claim query")
        .expect("pending job claimed");
    assert_eq!(claimed.job_id, first);
    assert_eq!(claimed.state, "claimed");
    assert!(claimed.claimed_at.is_some());
    assert_eq!(
        claimed.token,
        Fixture::read_state(&db.pool, first).await.token
    );

    assert!(
        claim_next_git_job(&db.pool, fixture.manager_id)
            .await
            .expect("second claim")
            .is_none(),
        "a claimed job is not reclaimable"
    );
    assert_eq!(Fixture::read_state(&db.pool, first).await.state, "claimed");

    // A foreign manager sees nothing and mutates nothing.
    let other_binding = fixture.add_binding(&db.pool, "code_project:foreign").await;
    let pending = seed(&db, &fixture, &other_binding, "pending").await;
    let (other_manager, _) = fixture.add_manager(&db.pool).await;
    assert!(
        claim_next_git_job(&db.pool, other_manager)
            .await
            .expect("foreign claim")
            .is_none(),
        "another manager cannot claim a job assigned elsewhere"
    );
    assert_eq!(
        Fixture::read_state(&db.pool, pending).await.state,
        "pending"
    );
    Fixture::set_deadline(&db.pool, pending, Utc::now() - ChronoDuration::seconds(1)).await;

    // Expired, already claimed, and uncertain rows never claim.
    let expired_binding = fixture.add_binding(&db.pool, "code_project:expired").await;
    let expired = fixture
        .seed_git_job(
            &db.pool,
            SeedJob {
                binding: &expired_binding,
                state: "pending",
                reserved_bytes: 1,
                deadline_at: Utc::now() - ChronoDuration::seconds(1),
                claimed_at: None,
                cleanup_confirmed: false,
                terminal_at: None,
            },
        )
        .await;
    let uncertain_binding = fixture
        .add_binding(&db.pool, "code_project:uncertain")
        .await;
    let uncertain = seed(&db, &fixture, &uncertain_binding, "uncertain").await;
    let claimed_binding = fixture.add_binding(&db.pool, "code_project:claimed").await;
    let already = seed(&db, &fixture, &claimed_binding, "claimed").await;

    assert!(
        claim_next_git_job(&db.pool, fixture.manager_id)
            .await
            .expect("expired claim")
            .is_none(),
        "expired, claimed, and uncertain rows are not claimable"
    );
    assert_eq!(
        Fixture::read_state(&db.pool, expired).await.state,
        "pending"
    );
    assert_eq!(
        Fixture::read_state(&db.pool, uncertain).await.state,
        "uncertain"
    );
    assert_eq!(
        Fixture::read_state(&db.pool, already).await.state,
        "claimed"
    );

    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn registered_poll_returns_the_secret_free_git_operation_wire_job() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let fixture = Fixture::setup(&db.pool).await;
    let harness = ManagerHarness::setup(db.pool.clone()).await;
    let binding = fixture.bindings[0].clone();
    let job_id = seed(&db, &fixture, &binding, "pending").await;
    let row = find_git_job_by_id(&db.pool, job_id)
        .await
        .expect("read pending row")
        .expect("pending row exists");

    let response = harness
        .get(
            "/api/internal/runner-manager/jobs/next",
            Some(&fixture.manager_token),
            &[],
        )
        .await;
    assert_eq!(response.status, 200, "poll: {}", response.body);
    let job: RunnerJob = serde_json::from_str(&response.body).expect("RunnerJob body");
    assert_eq!(job.job_id, format!("gitjob:{job_id}"));
    assert_eq!(job.kind, "git_operation");
    let spec: GitJobSpec = serde_json::from_value(job.payload.clone()).expect("GitJobSpec");
    spec.validate().expect("valid claim spec");
    assert_eq!(spec.job_id, job_id);
    assert_eq!(spec.operation_id, row.operation_id);
    assert_eq!(
        spec.owner,
        runner_protocol::RunnerOwner::ApplicationPrincipal {
            application_id: fixture.application_id,
            workspace_binding_id: binding.binding_id,
            principal_digest: fixture.principal_digest_for(&binding),
        }
    );
    assert_eq!(spec.workspace_root, binding_root(&binding, &db).await);
    assert_eq!(spec.deadline_unix_ms, row.deadline_at.timestamp_millis());

    let payload = serde_json::to_string(&job.payload).expect("payload text");
    for forbidden in [
        "password",
        "remote_url",
        "userinfo",
        &row.token,
        "smcpw1",
        "token",
    ] {
        assert!(
            !payload.contains(forbidden),
            "claim payload must not contain {forbidden:?}: {payload}"
        );
    }

    let after = Fixture::read_state(&db.pool, job_id).await;
    assert_eq!(after.state, "claimed");
    fixture.cleanup(&db.pool).await;
}

async fn binding_root(binding: &Binding, db: &IssueDb) -> String {
    desk_foreman::db::queries::find_workspace_binding_by_id(&db.pool, binding.binding_id)
        .await
        .expect("read binding")
        .expect("binding exists")
        .workspace_root
}
