//! Real-HTTP cutover cases for the credential-free Git sync route.
//!
//! The route is exercised through the registered router against the owned issue
//! database. A fake runner supplies only fixed probe output and a fake manager
//! claims/completes durable jobs; no Docker daemon, container, broker network,
//! or production system is touched.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use desk_foreman::config::GitJobSettings;
use desk_foreman::db::types::CreateApplicationTokenRequest;
use desk_foreman::db::workspace_git_jobs::lifecycle::{
    claim_next_git_job, complete_git_job, expire_git_jobs,
};
use desk_foreman::db::workspace_git_jobs::types::{GitJobCompletion, GitJobRecord, GitJobResult};
use desk_foreman::runner::{RunnerFuture, RunnerService};
use runner_protocol::{
    CancelSessionRequest, CommandOutput, ExecRequest, GitJobErrorCode, InputRequest,
    RunnerCommandRequest, RunnerOwner, RunnerSessionStatus, ShellToolOutput,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::fixtures::{Binding, Fixture, HttpResponse, RouteHarness};
use super::support::{IssueDb, SUITE_LOCK};

const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

#[derive(Default)]
struct FakeRunner {
    outputs: Mutex<Vec<CommandOutput>>,
    requests: Mutex<Vec<RunnerCommandRequest>>,
    delay: Mutex<Duration>,
}

impl FakeRunner {
    fn push(&self, stdout: &str, exit_code: i32) {
        self.outputs.lock().unwrap().push(CommandOutput {
            wall_time_seconds: 0.0,
            output: stdout.to_owned(),
            stdout: stdout.to_owned(),
            stderr: String::new(),
            exit_code: Some(exit_code),
            truncated: false,
            timed_out: false,
            stdout_bytes: stdout.len(),
            stderr_bytes: 0,
        });
    }

    fn requests(&self) -> Vec<RunnerCommandRequest> {
        self.requests.lock().unwrap().clone()
    }

    fn set_delay(&self, delay: Duration) {
        *self.delay.lock().unwrap() = delay;
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
        let delay = *self.delay.lock().unwrap();
        Box::pin(async move {
            tokio::time::sleep(delay).await;
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

fn secret_url(code_project_id: Uuid) -> String {
    format!(
        "https://broker:smcpw1.{}.{}@broker.example.invalid/api/internal/code-projects/{code_project_id}/git",
        Uuid::new_v4(),
        "a".repeat(64)
    )
}

fn body(binding: &Binding, remote_url: &str, generation: Uuid, preserve: bool) -> Value {
    json!({
        "remote_url": remote_url,
        "branch": "main",
        "preserve_existing": preserve,
        "code_project_id": binding.code_project_id,
        "authorization_generation": generation,
    })
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

async fn post(
    harness: &RouteHarness,
    fixture: &Fixture,
    binding: &Binding,
    token: &str,
    body: &Value,
) -> HttpResponse {
    harness
        .post_raw(
            &sync_path(binding),
            token,
            &headers(fixture, binding),
            &body.to_string(),
        )
        .await
}

async fn application_token(pool: &sqlx::PgPool, application_id: i64) -> String {
    desk_foreman::db::queries::create_application_token(
        pool,
        &CreateApplicationTokenRequest {
            application_id,
            token_name: format!("cutover-{}", Uuid::new_v4().simple()),
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
    .expect("create application token")
    .0
}

fn write_git_config(binding_root: &std::path::Path, config: &str) {
    let git = binding_root.join(".git");
    std::fs::create_dir_all(&git).expect("git dir");
    std::fs::write(git.join("config"), config).expect("git config");
}

async fn claim_with_retry(pool: &sqlx::PgPool, manager_id: i64) -> GitJobRecord {
    for _ in 0..400 {
        if let Some(job) = claim_next_git_job(pool, manager_id)
            .await
            .expect("claim query")
        {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("durable job was not claimable");
}

async fn binding_root(pool: &sqlx::PgPool, binding: &Binding) -> std::path::PathBuf {
    desk_foreman::db::queries::find_workspace_binding_by_id(pool, binding.binding_id)
        .await
        .expect("find binding")
        .expect("binding")
        .workspace_root
        .into()
}

#[tokio::test]
async fn route_rejects_mismatched_identity_and_malformed_secret_url_before_effects() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.add_derived_binding(&db.pool).await;
    let runner = Arc::new(FakeRunner::default());
    let harness = RouteHarness::setup(
        db.pool.clone(),
        fixture.workspace_root().to_path_buf(),
        None,
        runner.clone(),
    )
    .await;
    let token = application_token(&db.pool, fixture.application_id).await;
    let url = secret_url(binding.code_project_id);

    let mut mismatched = body(&binding, &url, Uuid::new_v4(), true);
    mismatched["code_project_id"] = json!(Uuid::new_v4());
    assert_eq!(
        post(&harness, &fixture, &binding, &token, &mismatched)
            .await
            .status,
        400
    );
    let nil_generation = body(&binding, &url, Uuid::nil(), true);
    assert_eq!(
        post(&harness, &fixture, &binding, &token, &nil_generation)
            .await
            .status,
        400
    );
    let malformed = body(
        &binding,
        "https://broker:not-a-secret@broker.example.invalid/api/internal/code-projects/x/git",
        Uuid::new_v4(),
        true,
    );
    assert_eq!(
        post(&harness, &fixture, &binding, &token, &malformed)
            .await
            .status,
        400
    );
    assert!(runner.requests().is_empty());
    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn route_reports_clean_preserve_without_network_or_secret_arguments() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.add_derived_binding(&db.pool).await;
    let root = binding_root(&db.pool, &binding).await;
    write_git_config(&root, "[core]\n\trepositoryformatversion = 0\n");
    let runner = Arc::new(FakeRunner::default());
    runner.push(&format!("SAFE\nHEAD {HEAD}\nBRANCH main\nCLEAN\n"), 0);
    let harness = RouteHarness::setup(
        db.pool.clone(),
        fixture.workspace_root().to_path_buf(),
        None,
        runner.clone(),
    )
    .await;
    let token = application_token(&db.pool, fixture.application_id).await;
    let response = post(
        &harness,
        &fixture,
        &binding,
        &token,
        &body(
            &binding,
            &secret_url(binding.code_project_id),
            Uuid::new_v4(),
            true,
        ),
    )
    .await;
    assert_eq!(response.status, 200);
    assert_eq!(response.json()["head_commit"], HEAD);
    assert_eq!(response.json()["cloned"], false);
    let requests = runner.requests();
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].network_enabled);
    assert!(!requests[0].args.iter().any(|arg| arg.contains("smcpw1")));
    assert!(!format!("{requests:?}").contains("smcpw1"));
    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn route_rejects_legacy_residue_includes_and_unsupported_shapes() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.add_derived_binding(&db.pool).await;
    let root = binding_root(&db.pool, &binding).await;
    write_git_config(&root, "[include]\n\tpath = other\n");
    let runner = Arc::new(FakeRunner::default());
    runner.push("CONFIG_UNSUPPORTED\n", 0);
    let harness = RouteHarness::setup(
        db.pool.clone(),
        fixture.workspace_root().to_path_buf(),
        None,
        runner.clone(),
    )
    .await;
    let token = application_token(&db.pool, fixture.application_id).await;
    let response = post(
        &harness,
        &fixture,
        &binding,
        &token,
        &body(
            &binding,
            &secret_url(binding.code_project_id),
            Uuid::new_v4(),
            true,
        ),
    )
    .await;
    assert_eq!(response.status, 409);
    assert_eq!(response.json()["error"], "legacy_git_cleanup_required");
    assert!(!response.body.contains("smcpw1"));

    runner.push("RESIDUE\n", 0);
    let response = post(
        &harness,
        &fixture,
        &binding,
        &token,
        &body(
            &binding,
            &secret_url(binding.code_project_id),
            Uuid::new_v4(),
            true,
        ),
    )
    .await;
    assert_eq!(response.status, 409);
    assert_eq!(response.json()["error"], "legacy_git_cleanup_required");
    assert!(root.join(".git").is_dir(), ".git is left untouched");
    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn route_refuses_existing_checkout_without_preserve_and_non_repo_target() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.add_derived_binding(&db.pool).await;
    let root = binding_root(&db.pool, &binding).await;
    std::fs::create_dir(root.join(".git")).expect("git dir");
    let runner = Arc::new(FakeRunner::default());
    let harness = RouteHarness::setup(
        db.pool.clone(),
        fixture.workspace_root().to_path_buf(),
        None,
        runner.clone(),
    )
    .await;
    let token = application_token(&db.pool, fixture.application_id).await;
    let response = post(
        &harness,
        &fixture,
        &binding,
        &token,
        &body(
            &binding,
            &secret_url(binding.code_project_id),
            Uuid::new_v4(),
            false,
        ),
    )
    .await;
    assert_eq!(response.status, 409);
    assert!(runner.requests().is_empty());

    std::fs::remove_dir(root.join(".git")).expect("remove git");
    std::fs::write(root.join("ordinary"), b"keep").expect("ordinary file");
    let response = post(
        &harness,
        &fixture,
        &binding,
        &token,
        &body(
            &binding,
            &secret_url(binding.code_project_id),
            Uuid::new_v4(),
            true,
        ),
    )
    .await;
    assert_eq!(response.status, 409);
    assert_eq!(std::fs::read(root.join("ordinary")).unwrap(), b"keep");
    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn route_returns_unavailable_when_git_jobs_are_not_configured() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.add_derived_binding(&db.pool).await;
    let runner = Arc::new(FakeRunner::default());
    let harness = RouteHarness::setup(
        db.pool.clone(),
        fixture.workspace_root().to_path_buf(),
        None,
        runner.clone(),
    )
    .await;
    let token = application_token(&db.pool, fixture.application_id).await;
    let response = post(
        &harness,
        &fixture,
        &binding,
        &token,
        &body(
            &binding,
            &secret_url(binding.code_project_id),
            Uuid::new_v4(),
            true,
        ),
    )
    .await;
    assert_eq!(response.status, 503);
    assert!(runner.requests().is_empty());
    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn first_clone_admits_to_the_observed_manager_and_reports_the_closed_result() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.add_derived_binding(&db.pool).await;
    fixture
        .seed_observation(&db.pool, &binding, "running")
        .await
        .expect("observation");
    let runner = Arc::new(FakeRunner::default());
    let harness = RouteHarness::setup(
        db.pool.clone(),
        fixture.workspace_root().to_path_buf(),
        Some(settings(3)),
        runner.clone(),
    )
    .await;
    let token = application_token(&db.pool, fixture.application_id).await;
    let pool = db.pool.clone();
    let manager_id = fixture.manager_id;
    let worker = tokio::spawn(async move {
        let claimed = claim_with_retry(&pool, manager_id).await;
        complete_git_job(
            &pool,
            claimed.job_id,
            &claimed.token,
            manager_id,
            GitJobCompletion::Succeeded {
                result: GitJobResult::SyncSucceeded {
                    head: HEAD.to_owned(),
                    base: None,
                    published: true,
                },
            },
        )
        .await
        .expect("complete");
        claimed
    });
    let response = post(
        &harness,
        &fixture,
        &binding,
        &token,
        &body(
            &binding,
            &secret_url(binding.code_project_id),
            Uuid::new_v4(),
            false,
        ),
    )
    .await;
    assert_eq!(response.status, 200);
    assert_eq!(response.json()["cloned"], true);
    assert_eq!(response.json()["dirty"], false);
    assert_eq!(response.json()["branch"], "main");
    assert_eq!(response.json()["head_commit"], HEAD);
    let claimed = worker.await.expect("worker");
    assert_eq!(claimed.assigned_manager_id, fixture.manager_id);
    assert_eq!(claimed.state, "claimed");
    assert_eq!(claimed.principal_digest.len(), 64);
    assert!(runner.requests().is_empty());
    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn assignment_bootstraps_once_then_unavailable_and_ambiguity_refuses() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.add_derived_binding(&db.pool).await;
    let runner = Arc::new(FakeRunner::default());
    runner.push("git version 2.47.0\n", 0);
    let harness = RouteHarness::setup(
        db.pool.clone(),
        fixture.workspace_root().to_path_buf(),
        Some(settings(1)),
        runner.clone(),
    )
    .await;
    let token = application_token(&db.pool, fixture.application_id).await;
    let response = post(
        &harness,
        &fixture,
        &binding,
        &token,
        &body(
            &binding,
            &secret_url(binding.code_project_id),
            Uuid::new_v4(),
            false,
        ),
    )
    .await;
    assert_eq!(response.status, 503);
    let requests = runner.requests();
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].network_enabled);
    assert!(requests[0].args.iter().any(|arg| arg == "git"));
    assert!(requests[0].args.iter().any(|arg| arg == "--version"));
    assert!(!requests[0].args.iter().any(|arg| arg.contains("smcpw1")));

    fixture
        .seed_observation(&db.pool, &binding, "running")
        .await
        .expect("first observation");
    let (second_manager, _) = fixture.add_manager(&db.pool).await;
    fixture
        .seed_observation_for(&db.pool, &binding, "running", second_manager)
        .await
        .expect("second observation");
    let response = post(
        &harness,
        &fixture,
        &binding,
        &token,
        &body(
            &binding,
            &secret_url(binding.code_project_id),
            Uuid::new_v4(),
            false,
        ),
    )
    .await;
    assert_eq!(response.status, 503);
    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn concurrent_second_request_is_busy_without_deadlock() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.add_derived_binding(&db.pool).await;
    fixture
        .seed_observation(&db.pool, &binding, "running")
        .await
        .expect("observation");
    let runner = Arc::new(FakeRunner::default());
    let harness = Arc::new(
        RouteHarness::setup(
            db.pool.clone(),
            fixture.workspace_root().to_path_buf(),
            Some(settings(4)),
            runner,
        )
        .await,
    );
    let token = application_token(&db.pool, fixture.application_id).await;
    let pool = db.pool.clone();
    let manager_id = fixture.manager_id;
    let (claimed_tx, claimed_rx) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(async move {
        let claimed = claim_with_retry(&pool, manager_id).await;
        let _ = claimed_tx.send(());
        tokio::time::sleep(Duration::from_millis(400)).await;
        complete_git_job(
            &pool,
            claimed.job_id,
            &claimed.token,
            manager_id,
            GitJobCompletion::Succeeded {
                result: GitJobResult::SyncSucceeded {
                    head: HEAD.to_owned(),
                    base: None,
                    published: true,
                },
            },
        )
        .await
        .expect("complete");
    });
    let path = sync_path(&binding);
    let owned_headers = headers(&fixture, &binding);
    let first_body = body(
        &binding,
        &secret_url(binding.code_project_id),
        Uuid::new_v4(),
        false,
    )
    .to_string();
    let first = {
        let harness = Arc::clone(&harness);
        let path = path.clone();
        let token = token.clone();
        let headers = owned_headers.clone();
        tokio::spawn(async move {
            harness
                .post_raw(&path, &token, &headers, &first_body)
                .await
                .status
        })
    };
    claimed_rx.await.expect("first request admitted");
    let second_body = body(
        &binding,
        &secret_url(binding.code_project_id),
        Uuid::new_v4(),
        false,
    );
    let second = harness
        .post_raw(&path, &token, &owned_headers, &second_body.to_string())
        .await;
    assert_eq!(
        second.status, 200,
        "polls the same job instead of admitting a new one"
    );
    assert_eq!(first.await.expect("first"), 200);
    worker.await.expect("worker");
    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn timeout_keeps_the_durable_job_and_never_replays() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.add_derived_binding(&db.pool).await;
    fixture
        .seed_observation(&db.pool, &binding, "running")
        .await
        .expect("observation");
    let runner = Arc::new(FakeRunner::default());
    let harness = RouteHarness::setup(
        db.pool.clone(),
        fixture.workspace_root().to_path_buf(),
        Some(settings(1)),
        runner,
    )
    .await;
    let token = application_token(&db.pool, fixture.application_id).await;
    let request = body(
        &binding,
        &secret_url(binding.code_project_id),
        Uuid::new_v4(),
        false,
    );
    let response = post(&harness, &fixture, &binding, &token, &request).await;
    assert_eq!(response.status, 503);
    let response = post(&harness, &fixture, &binding, &token, &request).await;
    assert_eq!(
        response.status, 503,
        "the retained job is still in progress"
    );
    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn fixed_failure_summaries_map_to_closed_statuses() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let runner = Arc::new(FakeRunner::default());
    let harness = RouteHarness::setup(
        db.pool.clone(),
        fixture.workspace_root().to_path_buf(),
        Some(settings(3)),
        runner,
    )
    .await;
    let token = application_token(&db.pool, fixture.application_id).await;
    let cases = [
        (GitJobErrorCode::Unavailable, 503),
        (GitJobErrorCode::CredentialUnavailable, 503),
        (GitJobErrorCode::Timeout, 503),
        (GitJobErrorCode::Cancelled, 409),
        (GitJobErrorCode::ResourceLimit, 503),
        (GitJobErrorCode::WorkspaceBusy, 409),
        (GitJobErrorCode::Conflict, 409),
        (GitJobErrorCode::ExecutionFailed, 503),
    ];
    for (code, expected) in cases {
        let binding = fixture.add_derived_binding(&db.pool).await;
        fixture
            .seed_observation(&db.pool, &binding, "running")
            .await
            .expect("observation");
        let pool = db.pool.clone();
        let manager_id = fixture.manager_id;
        let worker = tokio::spawn(async move {
            let claimed = claim_with_retry(&pool, manager_id).await;
            complete_git_job(
                &pool,
                claimed.job_id,
                &claimed.token,
                manager_id,
                GitJobCompletion::Failed {
                    error_summary: code.summary().to_owned(),
                    cleanup_confirmed: true,
                },
            )
            .await
            .expect("complete");
        });
        let response = post(
            &harness,
            &fixture,
            &binding,
            &token,
            &body(
                &binding,
                &secret_url(binding.code_project_id),
                Uuid::new_v4(),
                false,
            ),
        )
        .await;
        assert_eq!(response.status, expected, "summary {code:?}");
        assert!(!response.body.contains(code.summary()));
        worker.await.expect("worker");
    }

    let binding = fixture.add_derived_binding(&db.pool).await;
    fixture
        .seed_observation(&db.pool, &binding, "running")
        .await
        .expect("observation");
    let pool = db.pool.clone();
    let manager_id = fixture.manager_id;
    let worker = tokio::spawn(async move {
        let claimed = claim_with_retry(&pool, manager_id).await;
        complete_git_job(
            &pool,
            claimed.job_id,
            &claimed.token,
            manager_id,
            GitJobCompletion::Failed {
                error_summary: "arbitrary fixture failure".to_owned(),
                cleanup_confirmed: true,
            },
        )
        .await
        .expect("complete");
    });
    let response = post(
        &harness,
        &fixture,
        &binding,
        &token,
        &body(
            &binding,
            &secret_url(binding.code_project_id),
            Uuid::new_v4(),
            false,
        ),
    )
    .await;
    assert_eq!(response.status, 503);
    assert!(!response.body.contains("arbitrary fixture failure"));
    worker.await.expect("worker");

    let binding = fixture.add_derived_binding(&db.pool).await;
    fixture
        .seed_observation(&db.pool, &binding, "running")
        .await
        .expect("observation");
    let pool = db.pool.clone();
    let manager_id = fixture.manager_id;
    let worker = tokio::spawn(async move {
        let claimed = claim_with_retry(&pool, manager_id).await;
        complete_git_job(
            &pool,
            claimed.job_id,
            &claimed.token,
            manager_id,
            GitJobCompletion::Failed {
                error_summary: GitJobErrorCode::Unavailable.summary().to_owned(),
                cleanup_confirmed: false,
            },
        )
        .await
        .expect("complete");
    });
    let response = post(
        &harness,
        &fixture,
        &binding,
        &token,
        &body(
            &binding,
            &secret_url(binding.code_project_id),
            Uuid::new_v4(),
            false,
        ),
    )
    .await;
    assert_eq!(response.status, 503);
    worker.await.expect("worker");
    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn bootstrap_wait_is_bounded_by_the_single_route_deadline() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.add_derived_binding(&db.pool).await;
    let runner = Arc::new(FakeRunner::default());
    runner.push("git version 2.47.0\n", 0);
    runner.set_delay(Duration::from_millis(1500));
    let mut bounded = settings(2);
    bounded.wait = Duration::from_secs(2);
    bounded.deadline = Duration::from_secs(10);
    let harness = RouteHarness::setup(
        db.pool.clone(),
        fixture.workspace_root().to_path_buf(),
        Some(bounded),
        runner,
    )
    .await;
    let token = application_token(&db.pool, fixture.application_id).await;
    let started = Instant::now();
    let response = post(
        &harness,
        &fixture,
        &binding,
        &token,
        &body(
            &binding,
            &secret_url(binding.code_project_id),
            Uuid::new_v4(),
            false,
        ),
    )
    .await;
    let elapsed = started.elapsed();
    assert_eq!(response.status, 503);
    assert!(
        elapsed < Duration::from_millis(2600),
        "route exceeded its single wait deadline: {elapsed:?}"
    );
    assert!(
        claim_next_git_job(&db.pool, fixture.manager_id)
            .await
            .expect("claim query")
            .is_none(),
        "no job may be admitted after the original wait budget is exhausted"
    );
    fixture.cleanup(&db.pool).await;
}

#[tokio::test]
async fn timeout_then_expiry_releases_the_binding_guard() {
    let _lock = SUITE_LOCK.lock().await;
    let db = IssueDb::setup().await;
    let mut fixture = Fixture::setup(&db.pool).await;
    let binding = fixture.add_derived_binding(&db.pool).await;
    fixture
        .seed_observation(&db.pool, &binding, "running")
        .await
        .expect("observation");
    let runner = Arc::new(FakeRunner::default());
    let mut short = settings(1);
    short.deadline = Duration::from_secs(1);
    let harness = RouteHarness::setup(
        db.pool.clone(),
        fixture.workspace_root().to_path_buf(),
        Some(short),
        runner,
    )
    .await;
    let token = application_token(&db.pool, fixture.application_id).await;
    let request = body(
        &binding,
        &secret_url(binding.code_project_id),
        Uuid::new_v4(),
        false,
    );
    assert_eq!(
        post(&harness, &fixture, &binding, &token, &request)
            .await
            .status,
        503
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    let expired = expire_git_jobs(&db.pool).await.expect("expire");
    assert!(expired.iter().any(|row| {
        row.application_id == fixture.application_id
            && row.state == "failed"
            && row.cleanup_confirmed
    }));
    assert_eq!(
        post(&harness, &fixture, &binding, &token, &request)
            .await
            .status,
        503,
        "expired work no longer blocks a fresh admission"
    );
    fixture.cleanup(&db.pool).await;
}
