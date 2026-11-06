//! Manager authentication, credential delivery, and result mapping cases.

use chrono::{Duration as ChronoDuration, Utc};
use desk_foreman::db::workspace_git_jobs::store::find_git_job_by_id;
use desk_foreman::db::workspace_git_jobs::types::GitJobRecord;
use runner_protocol::{
    GIT_JOB_CREDENTIAL_USERNAME, GitJobCredential, GitJobErrorCode, GitJobOutcome, RunnerJobResult,
};
use uuid::Uuid;

use super::fixtures::{
    Binding, Fixture, HttpResponse, ManagerHarness, SeedJob, credential_identity,
    insert_credential, synthetic_credential,
};
use super::support::{IssueDb, SUITE_LOCK};

async fn claimed_job(
    db: &IssueDb,
    fixture: &mut Fixture,
    key: &str,
    deadline: chrono::DateTime<Utc>,
) -> (Binding, GitJobRecord) {
    let binding = fixture.add_binding(&db.pool, key).await;
    let job_id = fixture
        .seed_git_job(
            &db.pool,
            SeedJob {
                binding: &binding,
                state: "claimed",
                reserved_bytes: 64,
                deadline_at: deadline,
                claimed_at: Some(Utc::now()),
                cleanup_confirmed: false,
                terminal_at: None,
            },
        )
        .await;
    let row = find_git_job_by_id(&db.pool, job_id)
        .await
        .expect("read claimed row")
        .expect("claimed row exists");
    (binding, row)
}

fn credential_path(manager_id: i64, job_id: Uuid) -> String {
    format!("/api/internal/runner-managers/{manager_id}/git-jobs/{job_id}/credential")
}

fn result_body(job_id: Uuid, ok: bool, outcome: &GitJobOutcome) -> Vec<u8> {
    serde_json::to_vec(&RunnerJobResult {
        job_id: format!("gitjob:{job_id}"),
        ok,
        result: Some(serde_json::to_value(outcome).expect("outcome json")),
        error: None,
    })
    .expect("result body")
}

fn success(operation_id: Uuid) -> GitJobOutcome {
    GitJobOutcome::SyncSucceeded {
        operation_id,
        head: "a".repeat(40),
        base: None,
        published: true,
        cleanup_confirmed: true,
    }
}

fn failure(operation_id: Uuid, code: GitJobErrorCode, cleanup: bool) -> GitJobOutcome {
    GitJobOutcome::Failed {
        operation_id,
        error_code: code,
        cleanup_confirmed: cleanup,
    }
}

#[tokio::test]
async fn credential_endpoint_enforces_manager_path_operation_state_and_store_scope() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let harness = ManagerHarness::setup(db.pool.clone()).await;
    let (_binding, row) = claimed_job(
        &db,
        &mut fixture,
        "code_project:auth",
        Utc::now() + ChronoDuration::minutes(5),
    )
    .await;
    let credential =
        synthetic_credential(row.code_project_id, row.deadline_at.timestamp_millis(), 'c');
    insert_credential(harness.state(), &row, credential.clone());
    let path = credential_path(fixture.manager_id, row.job_id);
    let operation = row.operation_id.to_string();

    let missing = harness.get(&path, None, &[]).await;
    assert_eq!(missing.status, 401, "missing bearer is unauthenticated");
    let unknown = harness.get(&path, Some("unknown-token"), &[]).await;
    assert_eq!(unknown.status, 404, "unknown token discloses nothing");
    assert!(!unknown.body.contains(&credential.password));

    assert_eq!(
        harness
            .get(&path, Some(&fixture.manager_token), &[])
            .await
            .status,
        404,
        "missing operation header is refused"
    );
    assert_eq!(
        harness
            .get(
                &credential_path(fixture.manager_id + 1, row.job_id),
                Some(&fixture.manager_token),
                &[("x-df-git-operation-id", operation.as_str())],
            )
            .await
            .status,
        404,
        "path manager must equal the authenticated manager"
    );
    assert_eq!(
        harness
            .get(
                &path,
                Some(&fixture.manager_token),
                &[("x-df-git-operation-id", &Uuid::new_v4().to_string())],
            )
            .await
            .status,
        404,
        "a wrong operation id is refused without evicting the entry"
    );
    assert_eq!(
        harness
            .get(
                &path,
                Some(&fixture.manager_token),
                &[("x-df-git-operation-id", &operation)],
            )
            .await
            .status,
        200,
        "the wrong-scope request did not evict the matching entry"
    );

    let delivered: HttpResponse = harness
        .get(
            &path,
            Some(&fixture.manager_token),
            &[("x-df-git-operation-id", &operation)],
        )
        .await;
    assert_eq!(delivered.status, 200);
    assert_eq!(delivered.header("cache-control"), Some("no-store"));
    assert!(!delivered.body.contains('@'), "no URL userinfo");
    let body: GitJobCredential = serde_json::from_str(&delivered.body).expect("credential body");
    assert_eq!(body.username, GIT_JOB_CREDENTIAL_USERNAME);
    assert_eq!(body.password, credential.password);
    assert_eq!(body.remote_url, credential.remote_url);
    assert_eq!(body.deadline_unix_ms, credential.deadline_unix_ms);

    let redelivered = harness
        .get(
            &path,
            Some(&fixture.manager_token),
            &[("x-df-git-operation-id", &operation)],
        )
        .await;
    assert_eq!(
        redelivered.status, 200,
        "delivery is idempotent while claimed"
    );
    assert!(!redelivered.body.contains('@'));

    // A non-claimed row is never a credential scope.
    let pending_binding = fixture.add_binding(&db.pool, "code_project:pending").await;
    let pending_id = fixture
        .seed_git_job(
            &db.pool,
            SeedJob {
                binding: &pending_binding,
                state: "pending",
                reserved_bytes: 1,
                deadline_at: Utc::now() + ChronoDuration::minutes(5),
                claimed_at: None,
                cleanup_confirmed: false,
                terminal_at: None,
            },
        )
        .await;
    let pending_row = find_git_job_by_id(&db.pool, pending_id)
        .await
        .expect("read pending row")
        .expect("pending row");
    assert_eq!(
        harness
            .get(
                &credential_path(fixture.manager_id, pending_id),
                Some(&fixture.manager_token),
                &[(
                    "x-df-git-operation-id",
                    &pending_row.operation_id.to_string()
                )],
            )
            .await
            .status,
        404,
        "a pending row has no credential scope"
    );

    // Restart/missing store: the scope is valid but no credential is servable.
    harness
        .state()
        .git_job_credentials
        .remove(&credential_identity(&row));
    let missing_store = harness
        .get(
            &path,
            Some(&fixture.manager_token),
            &[("x-df-git-operation-id", &operation)],
        )
        .await;
    assert_eq!(missing_store.status, 409);
    assert_eq!(missing_store.json()["error"], "credential_unavailable");
    assert!(!missing_store.body.contains(&credential.password));

    insert_credential(harness.state(), &row, credential.clone());
    assert_eq!(
        harness
            .get(
                &path,
                Some(&fixture.manager_token),
                &[("x-df-git-operation-id", &operation)],
            )
            .await
            .status,
        200,
        "the explicit reinsert restores delivery"
    );

    assert!(!format!("{row:?}").contains(&credential.password));
    assert!(!format!("{credential:?}").contains(&credential.password));
    assert!(!format!("{:?}", harness.state().git_job_credentials).contains(&credential.password));

    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn credential_scope_rejects_expired_claimed_rows() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let harness = ManagerHarness::setup(db.pool.clone()).await;
    let (_binding, row) = claimed_job(
        &db,
        &mut fixture,
        "code_project:expired-auth",
        Utc::now() + ChronoDuration::minutes(5),
    )
    .await;
    let credential =
        synthetic_credential(row.code_project_id, row.deadline_at.timestamp_millis(), 'd');
    insert_credential(harness.state(), &row, credential);
    Fixture::set_deadline(
        &db.pool,
        row.job_id,
        Utc::now() - ChronoDuration::seconds(1),
    )
    .await;

    let response = harness
        .get(
            &credential_path(fixture.manager_id, row.job_id),
            Some(&fixture.manager_token),
            &[("x-df-git-operation-id", &row.operation_id.to_string())],
        )
        .await;
    assert_eq!(response.status, 404, "an expired claim has no scope");
    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn result_route_maps_closed_outcomes_and_removes_only_the_matched_credential() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let harness = ManagerHarness::setup(db.pool.clone()).await;

    let (_binding, row) = claimed_job(
        &db,
        &mut fixture,
        "code_project:result",
        Utc::now() + ChronoDuration::minutes(5),
    )
    .await;
    let credential =
        synthetic_credential(row.code_project_id, row.deadline_at.timestamp_millis(), 'e');
    insert_credential(harness.state(), &row, credential);
    let identity = credential_identity(&row);
    let result_path = "/api/internal/runner-manager/jobs/result";

    let malformed = harness
        .post_json(
            result_path,
            Some(&fixture.manager_token),
            &result_body(row.job_id, true, &success(row.operation_id)),
        )
        .await;
    assert_eq!(malformed.status, 204, "valid result");
    assert_eq!(
        Fixture::read_state(&db.pool, row.job_id).await.state,
        "succeeded"
    );
    assert!(harness.state().git_job_credentials.get(&identity).is_none());

    let replay = harness
        .post_json(
            result_path,
            Some(&fixture.manager_token),
            &result_body(row.job_id, true, &success(row.operation_id)),
        )
        .await;
    assert_eq!(replay.status, 204, "exact terminal replay is accepted");

    let contradiction = harness
        .post_json(
            result_path,
            Some(&fixture.manager_token),
            &result_body(
                row.job_id,
                false,
                &failure(row.operation_id, GitJobErrorCode::Conflict, true),
            ),
        )
        .await;
    assert_eq!(contradiction.status, 409, "contradictory replay is refused");

    let (_bind2, cleanup_row) = claimed_job(
        &db,
        &mut fixture,
        "code_project:cleanup",
        Utc::now() + ChronoDuration::minutes(5),
    )
    .await;
    let cleanup_credential = synthetic_credential(
        cleanup_row.code_project_id,
        cleanup_row.deadline_at.timestamp_millis(),
        'f',
    );
    insert_credential(harness.state(), &cleanup_row, cleanup_credential);
    let cleanup_false = harness
        .post_json(
            result_path,
            Some(&fixture.manager_token),
            &result_body(
                cleanup_row.job_id,
                true,
                &GitJobOutcome::SyncSucceeded {
                    operation_id: cleanup_row.operation_id,
                    head: "b".repeat(40),
                    base: None,
                    published: true,
                    cleanup_confirmed: false,
                },
            ),
        )
        .await;
    assert_eq!(cleanup_false.status, 204);
    let cleanup_state = Fixture::read_state(&db.pool, cleanup_row.job_id).await;
    assert_eq!(cleanup_state.state, "uncertain");
    assert!(!cleanup_state.cleanup_confirmed);
    assert!(
        harness
            .state()
            .git_job_credentials
            .get(&credential_identity(&cleanup_row))
            .is_none(),
        "leaving claimed removes the credential"
    );

    let (_bind3, bad_row) = claimed_job(
        &db,
        &mut fixture,
        "code_project:bad",
        Utc::now() + ChronoDuration::minutes(5),
    )
    .await;
    let bad_bodies = [
        (
            "wrong-operation",
            result_body(bad_row.job_id, true, &success(Uuid::new_v4())),
        ),
        (
            "wrong-ok",
            result_body(bad_row.job_id, false, &success(bad_row.operation_id)),
        ),
        (
            "missing-payload",
            serde_json::to_vec(&RunnerJobResult {
                job_id: format!("gitjob:{}", bad_row.job_id),
                ok: true,
                result: None,
                error: None,
            })
            .expect("body"),
        ),
        (
            "bad-head",
            result_body(
                bad_row.job_id,
                true,
                &GitJobOutcome::SyncSucceeded {
                    operation_id: bad_row.operation_id,
                    head: "A".repeat(40),
                    base: None,
                    published: true,
                    cleanup_confirmed: true,
                },
            ),
        ),
    ];
    for (name, body) in bad_bodies {
        let response = harness
            .post_json(result_path, Some(&fixture.manager_token), &body)
            .await;
        assert!(
            response.status == 400 || response.status == 404,
            "{name} must be rejected with no mutation, got {}",
            response.status
        );
        assert_eq!(
            Fixture::read_state(&db.pool, bad_row.job_id).await.state,
            "claimed",
            "{name} must not mutate the row"
        );
    }

    // A foreign manager and an unknown wire id disclose nothing.
    let (other_manager, other_token) = fixture.add_manager(&db.pool).await;
    let _ = other_manager;
    let foreign = harness
        .post_json(
            result_path,
            Some(&other_token),
            &result_body(bad_row.job_id, true, &success(bad_row.operation_id)),
        )
        .await;
    assert_eq!(foreign.status, 404);
    let unknown = harness
        .post_json(
            result_path,
            Some(&fixture.manager_token),
            &result_body(Uuid::new_v4(), true, &success(Uuid::new_v4())),
        )
        .await;
    assert_eq!(unknown.status, 404);

    // A failed outcome persists only the fixed bounded error summary.
    let (_bind4, failed_row) = claimed_job(
        &db,
        &mut fixture,
        "code_project:failed",
        Utc::now() + ChronoDuration::minutes(5),
    )
    .await;
    let failed = harness
        .post_json(
            result_path,
            Some(&fixture.manager_token),
            &result_body(
                failed_row.job_id,
                false,
                &failure(
                    failed_row.operation_id,
                    GitJobErrorCode::CredentialUnavailable,
                    true,
                ),
            ),
        )
        .await;
    assert_eq!(failed.status, 204);
    let stored = Fixture::read_state(&db.pool, failed_row.job_id).await;
    assert_eq!(stored.state, "failed");
    assert!(stored.cleanup_confirmed);

    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn a_pending_job_cannot_be_completed_by_the_result_route() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let harness = ManagerHarness::setup(db.pool.clone()).await;
    let binding = fixture
        .add_binding(&db.pool, "code_project:pending-result")
        .await;
    let job_id = fixture
        .seed_git_job(
            &db.pool,
            SeedJob {
                binding: &binding,
                state: "pending",
                reserved_bytes: 1,
                deadline_at: Utc::now() + ChronoDuration::minutes(5),
                claimed_at: None,
                cleanup_confirmed: false,
                terminal_at: None,
            },
        )
        .await;
    let row = find_git_job_by_id(&db.pool, job_id)
        .await
        .expect("read pending row")
        .expect("pending row");
    let response = harness
        .post_json(
            "/api/internal/runner-manager/jobs/result",
            Some(&fixture.manager_token),
            &result_body(job_id, true, &success(row.operation_id)),
        )
        .await;
    assert_eq!(response.status, 409);
    assert_eq!(Fixture::read_state(&db.pool, job_id).await.state, "pending");
    fixture.cleanup(&db.pool).await;
}
