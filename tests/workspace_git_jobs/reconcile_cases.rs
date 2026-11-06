//! Proof-only reconciliation and clean fast-forward route cases.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{Duration as ChronoDuration, Utc};
use desk_foreman::config::GitJobSettings;
use desk_foreman::db::types::CreateApplicationTokenRequest;
use desk_foreman::db::workspace_git_jobs::store::{
    GitJobReconcile, find_active_git_job_for_binding, find_git_job_by_id,
    reconcile_uncertain_git_job,
};
use desk_foreman::db::workspace_git_jobs::types::GitJobResult;
use desk_foreman::runner::{RunnerFuture, RunnerService};
use runner_protocol::{
    CancelSessionRequest, CommandOutput, ExecRequest, GitReceiptPhase, GitReceiptReadAck,
    GitReceiptReadRequest, GitReceiptState, InputRequest, RunnerCommandRequest, RunnerOwner,
    RunnerSessionStatus, ShellToolOutput,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::fixtures::{Binding, Fixture, HttpResponse, RouteHarness, SeedJob};
use super::support::{IssueDb, SUITE_LOCK};

const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

struct FakeRunner {
    outputs: Mutex<Vec<CommandOutput>>,
    ack: Mutex<Option<GitReceiptReadAck>>,
    requests: Mutex<Vec<RunnerCommandRequest>>,
    receipt_requests: Mutex<Vec<GitReceiptReadRequest>>,
}

impl FakeRunner {
    fn new() -> Self {
        Self {
            outputs: Mutex::new(Vec::new()),
            ack: Mutex::new(None),
            requests: Mutex::new(Vec::new()),
            receipt_requests: Mutex::new(Vec::new()),
        }
    }

    fn push_probe(&self) {
        let stdout = format!("SAFE\nHEAD {HEAD}\nBRANCH main\nCLEAN\n");
        self.outputs.lock().unwrap().push(CommandOutput {
            wall_time_seconds: 0.0,
            output: stdout.clone(),
            stdout,
            stderr: String::new(),
            exit_code: Some(0),
            truncated: false,
            timed_out: false,
            stdout_bytes: 0,
            stderr_bytes: 0,
        });
    }

    fn set_ack(&self, ack: GitReceiptReadAck) {
        *self.ack.lock().unwrap() = Some(ack);
    }

    fn requests(&self) -> Vec<RunnerCommandRequest> {
        self.requests.lock().unwrap().clone()
    }

    fn receipt_requests(&self) -> Vec<GitReceiptReadRequest> {
        self.receipt_requests.lock().unwrap().clone()
    }
}

impl RunnerService for FakeRunner {
    fn exec_shell<'a>(
        &'a self,
        _: ExecRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async { anyhow::bail!("unsupported") })
    }
    fn write_stdin<'a>(
        &'a self,
        _: InputRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async { anyhow::bail!("unsupported") })
    }
    fn cancel_session<'a>(
        &'a self,
        _: CancelSessionRequest,
    ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>> {
        Box::pin(async { anyhow::bail!("unsupported") })
    }
    fn list_sessions<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn run_command<'a>(
        &'a self,
        request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
        self.requests.lock().unwrap().push(request);
        Box::pin(async move {
            self.outputs
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| anyhow::anyhow!("unexpected runner command"))
        })
    }
    fn cleanup_runner_owner<'a>(&'a self, _: RunnerOwner) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn read_git_receipt<'a>(
        &'a self,
        request: GitReceiptReadRequest,
    ) -> RunnerFuture<'a, anyhow::Result<GitReceiptReadAck>> {
        self.receipt_requests.lock().unwrap().push(request);
        let ack = self.ack.lock().unwrap().clone();
        Box::pin(async move { ack.ok_or_else(|| anyhow::anyhow!("receipt unavailable")) })
    }
}

fn settings(wait_secs: u64) -> GitJobSettings {
    GitJobSettings {
        wait: Duration::from_secs(wait_secs),
        deadline: Duration::from_secs(wait_secs.max(10)),
        reserved_bytes: 4 * 1024 * 1024,
        max_active: 1024,
        max_bytes: i64::MAX / 4,
        max_per_binding: 4,
        max_per_manager: 64,
        max_bytes_per_manager: i64::MAX / 4,
    }
}

fn headers(fixture: &Fixture, binding: &Binding) -> Vec<(String, String)> {
    vec![
        ("X-DF-Runner-Isolation".to_owned(), "principal".to_owned()),
        (
            "X-DF-External-User-Id".to_owned(),
            fixture.lease_owner.clone(),
        ),
        (
            "X-DF-Workspace-Key".to_owned(),
            binding.workspace_key.clone(),
        ),
        ("X-DF-Lease-Owner".to_owned(), fixture.lease_owner.clone()),
    ]
}

fn sync_path(binding: &Binding) -> String {
    format!(
        "/api/internal/resource-workspaces/code_project/{}/git/sync",
        binding.code_project_id
    )
}

fn body(binding: &Binding) -> Value {
    json!({
        "remote_url": format!(
            "https://broker:smcpw1.{}.{}@broker.example.invalid/api/internal/code-projects/{}/git",
            Uuid::new_v4(),
            "a".repeat(64),
            binding.code_project_id
        ),
        "branch": "main",
        "preserve_existing": false,
        "code_project_id": binding.code_project_id,
        "authorization_generation": Uuid::new_v4(),
    })
}

async fn post(
    harness: &RouteHarness,
    fixture: &Fixture,
    binding: &Binding,
    token: &str,
) -> HttpResponse {
    harness
        .post_raw(
            &sync_path(binding),
            token,
            &headers(fixture, binding),
            &body(binding).to_string(),
        )
        .await
}

async fn application_token(pool: &sqlx::PgPool, application_id: i64) -> String {
    desk_foreman::db::queries::create_application_token(
        pool,
        &CreateApplicationTokenRequest {
            application_id,
            token_name: format!("reconcile-{}", Uuid::new_v4().simple()),
            expires_at: None,
            scopes: None,
            max_timeout_ms: None,
            max_output_bytes: None,
            max_file_bytes: None,
            max_sessions: None,
            network_enabled: Some(true),
        },
    )
    .await
    .expect("create token")
    .0
}

fn success_ack(
    row: &desk_foreman::db::workspace_git_jobs::types::GitJobRecord,
    head: &str,
) -> GitReceiptReadAck {
    GitReceiptReadAck {
        job_id: row.job_id,
        operation_id: row.operation_id,
        workspace_binding_id: row.workspace_binding_id,
        state: GitReceiptState::Confirmed,
        phase: Some(GitReceiptPhase::Completed),
        base: row.expected_base.clone(),
        head: Some(head.to_owned()),
        published: true,
        cleanup_confirmed: true,
    }
}

async fn seed_existing_git(db: &IssueDb, binding: &Binding) {
    let root: std::path::PathBuf =
        desk_foreman::db::queries::find_workspace_binding_by_id(&db.pool, binding.binding_id)
            .await
            .expect("binding")
            .expect("exists")
            .workspace_root
            .into();
    std::fs::create_dir_all(root.join(".git")).expect("git dir");
    std::fs::write(
        root.join(".git/config"),
        "[core]\n\trepositoryformatversion = 0\n",
    )
    .expect("git config");
}

async fn seed_uncertain(
    db: &IssueDb,
    fixture: &Fixture,
    binding: &Binding,
) -> desk_foreman::db::workspace_git_jobs::types::GitJobRecord {
    let job_id = fixture
        .seed_git_job_with_expected_base(
            &db.pool,
            SeedJob {
                binding,
                state: "uncertain",
                reserved_bytes: 64,
                deadline_at: Utc::now() + ChronoDuration::minutes(5),
                claimed_at: Some(Utc::now()),
                cleanup_confirmed: false,
                terminal_at: None,
            },
            HEAD,
        )
        .await;
    fixture
        .seed_observation(&db.pool, binding, "running")
        .await
        .expect("manager observation");
    find_git_job_by_id(&db.pool, job_id)
        .await
        .expect("read uncertain")
        .expect("uncertain exists")
}

#[tokio::test]
async fn store_exact_matching_proof_releases_reservation_and_stores_strict_result() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.add_derived_binding(&db.pool).await;
    let row = seed_uncertain(&db, &fixture, &binding).await;
    let updated = reconcile_uncertain_git_job(
        &db.pool,
        &GitJobReconcile {
            job_id: row.job_id,
            token: &row.token,
            assigned_manager_id: row.assigned_manager_id,
            operation_id: row.operation_id,
            application_id: row.application_id,
            workspace_binding_id: row.workspace_binding_id,
            expected_base: row.expected_base.as_deref(),
            canonical_branch: &row.canonical_branch,
            principal_digest: &row.principal_digest,
            head: HEAD,
        },
    )
    .await
    .expect("reconcile")
    .expect("updated");
    assert_eq!(updated.state, "succeeded");
    assert!(updated.cleanup_confirmed);
    assert!(updated.terminal_at.is_some());
    let result = updated.result.expect("result");
    let decoded: GitJobResult = serde_json::from_value(result).expect("strict result");
    assert_eq!(
        decoded,
        GitJobResult::SyncSucceeded {
            head: HEAD.to_owned(),
            base: Some(HEAD.to_owned()),
            published: true,
        }
    );
    fixture.cleanup(&db.pool).await;
}

fn base_reconcile<'a>(
    row: &'a desk_foreman::db::workspace_git_jobs::types::GitJobRecord,
) -> GitJobReconcile<'a> {
    GitJobReconcile {
        job_id: row.job_id,
        token: &row.token,
        assigned_manager_id: row.assigned_manager_id,
        operation_id: row.operation_id,
        application_id: row.application_id,
        workspace_binding_id: row.workspace_binding_id,
        expected_base: row.expected_base.as_deref(),
        canonical_branch: &row.canonical_branch,
        principal_digest: &row.principal_digest,
        head: HEAD,
    }
}

#[tokio::test]
async fn store_rejects_every_identity_or_result_mismatch_and_keeps_uncertain() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.add_derived_binding(&db.pool).await;
    let row = seed_uncertain(&db, &fixture, &binding).await;
    let wrong_base = "b".repeat(40);
    let wrong_principal = "c".repeat(64);
    let wrong_head = "A".repeat(40);
    let cases = vec![
        GitJobReconcile {
            job_id: Uuid::new_v4(),
            ..base_reconcile(&row)
        },
        GitJobReconcile {
            token: "wrong-token",
            ..base_reconcile(&row)
        },
        GitJobReconcile {
            assigned_manager_id: row.assigned_manager_id + 1,
            ..base_reconcile(&row)
        },
        GitJobReconcile {
            operation_id: Uuid::new_v4(),
            ..base_reconcile(&row)
        },
        GitJobReconcile {
            application_id: row.application_id + 1,
            ..base_reconcile(&row)
        },
        GitJobReconcile {
            workspace_binding_id: row.workspace_binding_id + 1,
            ..base_reconcile(&row)
        },
        GitJobReconcile {
            expected_base: Some(&wrong_base),
            ..base_reconcile(&row)
        },
        GitJobReconcile {
            canonical_branch: "other",
            ..base_reconcile(&row)
        },
        GitJobReconcile {
            principal_digest: &wrong_principal,
            ..base_reconcile(&row)
        },
        GitJobReconcile {
            head: &wrong_head,
            ..base_reconcile(&row)
        },
    ];
    for reconcile in &cases {
        let outcome = reconcile_uncertain_git_job(&db.pool, reconcile).await;
        assert!(
            matches!(outcome, Err(_) | Ok(None)),
            "mismatched proof must not update"
        );
    }
    let current = find_git_job_by_id(&db.pool, row.job_id)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(current.state, "uncertain");
    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn route_confirmed_completed_receipt_resolves_uncertain_without_new_job_or_credential() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.add_derived_binding(&db.pool).await;
    seed_existing_git(&db, &binding).await;
    let row = seed_uncertain(&db, &fixture, &binding).await;
    let runner = Arc::new(FakeRunner::new());
    runner.push_probe();
    runner.push_probe();
    runner.set_ack(success_ack(&row, HEAD));
    let harness = RouteHarness::setup(
        db.pool.clone(),
        fixture.workspace_root().to_path_buf(),
        Some(settings(2)),
        runner.clone(),
    )
    .await;
    let token = application_token(&db.pool, fixture.application_id).await;
    let response = post(&harness, &fixture, &binding, &token).await;
    assert_eq!(response.status, 200, "{}", response.body);
    assert_eq!(response.json()["head_commit"], HEAD);
    assert_eq!(response.json()["cloned"], false);
    let current = find_git_job_by_id(&db.pool, row.job_id)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(current.state, "succeeded");
    assert_eq!(runner.receipt_requests().len(), 1);
    assert_eq!(runner.receipt_requests()[0].job_id, row.job_id);
    assert!(!format!("{:?}", runner.receipt_requests()).contains("smcpw1"));
    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn route_missing_or_mismatched_receipt_stays_503_and_uncertain() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.add_derived_binding(&db.pool).await;
    let row = seed_uncertain(&db, &fixture, &binding).await;
    let runner = Arc::new(FakeRunner::new());
    runner.push_probe();
    runner.push_probe();
    runner.set_ack(GitReceiptReadAck {
        state: GitReceiptState::Missing,
        phase: None,
        base: row.expected_base.clone(),
        head: None,
        published: false,
        cleanup_confirmed: false,
        ..success_ack(&row, HEAD)
    });
    let harness = RouteHarness::setup(
        db.pool.clone(),
        fixture.workspace_root().to_path_buf(),
        Some(settings(1)),
        runner.clone(),
    )
    .await;
    let token = application_token(&db.pool, fixture.application_id).await;
    let response = post(&harness, &fixture, &binding, &token).await;
    assert_eq!(response.status, 503);
    let current = find_git_job_by_id(&db.pool, row.job_id)
        .await
        .expect("read")
        .expect("row");
    assert_eq!(current.state, "uncertain");
    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn route_repeated_pending_or_claimed_poll_never_admits_a_new_job() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.add_derived_binding(&db.pool).await;
    let job_id = fixture
        .seed_git_job_with_expected_base(
            &db.pool,
            SeedJob {
                binding: &binding,
                state: "claimed",
                reserved_bytes: 64,
                deadline_at: Utc::now() + ChronoDuration::minutes(5),
                claimed_at: Some(Utc::now()),
                cleanup_confirmed: false,
                terminal_at: None,
            },
            HEAD,
        )
        .await;
    let runner = Arc::new(FakeRunner::new());
    runner.push_probe();
    let harness = RouteHarness::setup(
        db.pool.clone(),
        fixture.workspace_root().to_path_buf(),
        Some(settings(1)),
        runner.clone(),
    )
    .await;
    let token = application_token(&db.pool, fixture.application_id).await;
    let response = post(&harness, &fixture, &binding, &token).await;
    assert_eq!(response.status, 503);
    let active =
        find_active_git_job_for_binding(&db.pool, binding.binding_id, fixture.application_id)
            .await
            .expect("active")
            .expect("still active");
    assert_eq!(active.job_id, job_id);
    assert_eq!(active.state, "claimed");
    assert!(runner.receipt_requests().is_empty());
    assert_eq!(
        runner.requests().len(),
        0,
        "no probe or new dispatch was needed"
    );
    let started = Instant::now();
    assert!(started.elapsed() < Duration::from_secs(5));
    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn route_default_clean_existing_admits_fast_forward_with_observed_head_base() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.add_derived_binding(&db.pool).await;
    seed_existing_git(&db, &binding).await;
    fixture
        .seed_observation(&db.pool, &binding, "running")
        .await
        .expect("manager observation");
    let runner = Arc::new(FakeRunner::new());
    runner.push_probe();
    let harness = RouteHarness::setup(
        db.pool.clone(),
        fixture.workspace_root().to_path_buf(),
        Some(settings(1)),
        runner.clone(),
    )
    .await;
    let token = application_token(&db.pool, fixture.application_id).await;
    let response = post(&harness, &fixture, &binding, &token).await;
    assert_eq!(response.status, 503, "{}", response.body);
    let active =
        find_active_git_job_for_binding(&db.pool, binding.binding_id, fixture.application_id)
            .await
            .expect("active")
            .expect("admitted");
    assert_eq!(active.expected_base.as_deref(), Some(HEAD));
    assert_eq!(active.canonical_branch, "main");
    assert_eq!(active.state, "pending");
    assert!(runner.receipt_requests().is_empty());
    fixture.cleanup(&db.pool).await;
}
