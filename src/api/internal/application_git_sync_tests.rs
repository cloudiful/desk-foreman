use super::redact_git_output;
use crate::db::types::WorkspaceBindingResponse;
use chrono::{Duration, Utc};

#[path = "../admin/lease_test_support.rs"]
mod support;

#[derive(Default)]
struct ScriptedRunner {
    replies:
        std::sync::Mutex<std::collections::VecDeque<(Vec<String>, runner_protocol::CommandOutput)>>,
    owners: std::sync::Mutex<Vec<runner_protocol::RunnerOwner>>,
    config: std::sync::Mutex<std::collections::BTreeMap<String, Vec<String>>>,
}

impl ScriptedRunner {
    fn add(&self, args: &[&str], stdout: &str, code: i32) {
        self.replies.lock().unwrap().push_back((
            args.iter().map(|s| s.to_string()).collect(),
            runner_protocol::CommandOutput {
                wall_time_seconds: 0.0,
                output: stdout.into(),
                stdout: stdout.into(),
                stderr: String::new(),
                exit_code: Some(code),
                truncated: false,
                timed_out: false,
                stdout_bytes: stdout.len(),
                stderr_bytes: 0,
            },
        ));
    }
    fn done(&self) {
        assert!(self.replies.lock().unwrap().is_empty());
    }
}

impl crate::runner::RunnerService for ScriptedRunner {
    fn exec_shell<'a>(
        &'a self,
        _: runner_protocol::ExecRequest,
    ) -> crate::runner::RunnerFuture<'a, anyhow::Result<runner_protocol::ShellToolOutput>> {
        Box::pin(async { anyhow::bail!("unsupported") })
    }
    fn write_stdin<'a>(
        &'a self,
        _: runner_protocol::InputRequest,
    ) -> crate::runner::RunnerFuture<'a, anyhow::Result<runner_protocol::ShellToolOutput>> {
        Box::pin(async { anyhow::bail!("unsupported") })
    }
    fn cancel_session<'a>(
        &'a self,
        _: runner_protocol::CancelSessionRequest,
    ) -> crate::runner::RunnerFuture<'a, anyhow::Result<runner_protocol::RunnerSessionStatus>> {
        Box::pin(async { anyhow::bail!("unsupported") })
    }
    fn list_sessions<'a>(
        &'a self,
    ) -> crate::runner::RunnerFuture<'a, anyhow::Result<Vec<runner_protocol::RunnerSessionStatus>>>
    {
        Box::pin(async { Ok(vec![]) })
    }
    fn cleanup_runner_owner<'a>(
        &'a self,
        _: runner_protocol::RunnerOwner,
    ) -> crate::runner::RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn run_command<'a>(
        &'a self,
        request: runner_protocol::RunnerCommandRequest,
    ) -> crate::runner::RunnerFuture<'a, anyhow::Result<runner_protocol::CommandOutput>> {
        Box::pin(async move {
            self.owners.lock().unwrap().push(request.owner);
            let (args, output) = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected git command");
            assert_eq!(request.args, args);
            if output.exit_code == Some(0) && args.first().is_some_and(|v| v == "config") {
                self.config
                    .lock()
                    .unwrap()
                    .insert(args[3].clone(), vec![args[4].clone()]);
            }
            Ok(output)
        })
    }
}

#[test]
fn request_defaults_to_legacy_and_redacts_credentials() {
    let request: crate::db::types::GitWorkspaceSyncRequest =
        serde_json::from_str(r#"{"remote_url":"https://example.invalid"}"#).unwrap();
    assert!(!request.preserve_existing);
    let output = redact_git_output(
        "fatal https://user:secret@example.invalid/path and HTTP://other:password@host/x\nAuthorization: Bearer private",
    );
    for secret in ["user:secret", "other:password", "Bearer private"] {
        assert!(!output.contains(secret));
    }
    assert!(output.contains("[redacted]"));
}

#[tokio::test]
#[ignore = "requires newly initialized disposable P4n PostgreSQL"]
async fn sync_scripts_preserve_state_and_guard_identity() -> anyhow::Result<()> {
    use crate::db::{
        queries,
        types::{CreateApplicationTokenRequest, GitWorkspaceSyncRequest},
    };
    use axum::{
        extract::{Path, State},
        http::HeaderMap,
    };
    use std::sync::Arc;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&std::env::var("P4N_TEST_DATABASE_URL")?)
        .await?;
    sqlx::raw_sql(include_str!(
        "../../sql/principal_admission/test_lifecycle_seed.sql"
    ))
    .execute(&pool)
    .await?;
    sqlx::query(include_str!(
        "../../sql/principal_admission/test_p4n_network.sql"
    ))
    .execute(&pool)
    .await?;
    let (token, _) = queries::create_application_token(
        &pool,
        &CreateApplicationTokenRequest {
            application_id: 817301,
            token_name: "p4n".into(),
            expires_at: None,
            scopes: None,
            max_timeout_ms: None,
            max_output_bytes: None,
            max_file_bytes: None,
            max_sessions: None,
            network_enabled: Some(true),
        },
    )
    .await?;
    let temp = tempfile::tempdir()?;
    let mut state = support::fake_state(support::FakeRunnerService::new());
    state.db = pool.clone();
    let mut config = (*state.config).clone();
    config.workspace_root = temp.path().into();
    state.config = Arc::new(config);
    let runner = Arc::new(ScriptedRunner::default());
    runner.config.lock().unwrap().insert(
        "remote.origin.url".into(),
        vec!["https://old:unrestricted@example.invalid/repo".into()],
    );
    runner.config.lock().unwrap().insert(
        "remote.origin.pushurl".into(),
        vec![
            "https://old:unrestricted@example.invalid/repo".into(),
            "https://another:unrestricted@example.invalid/repo".into(),
        ],
    );
    state.runner = runner.clone();
    let application = queries::find_application_by_id(&pool, 817301)
        .await?
        .unwrap();
    let binding = crate::auth::identity::resolve_or_create_resource_binding(
        &state,
        &application,
        "code_project",
        "one",
        "code_project:one",
    )
    .await
    .unwrap();
    queries::acquire_workspace_write_lease(&pool, binding.workspace_binding_id, "principal", 300)
        .await?
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("authorization", format!("Bearer {token}").parse()?);
    headers.insert("x-df-runner-isolation", "principal".parse()?);
    headers.insert("x-df-external-user-id", "principal".parse()?);
    headers.insert("x-df-workspace-key", "code_project:one".parse()?);
    headers.insert("x-df-lease-owner", "principal".parse()?);
    let remote = "https://limited:token@example.invalid/repo";
    let call = |preserve_existing, remote_url: &str| {
        super::sync_resource_workspace_git(
            State(state.clone()),
            headers.clone(),
            Path(("code_project".to_string(), "one".to_string())),
            crate::api::validation::ValidatedJson(GitWorkspaceSyncRequest {
                remote_url: remote_url.into(),
                branch: None,
                preserve_existing,
            }),
        )
    };
    let tail = |remote: &str, dirty: &str, branch: &str| {
        runner.add(
            &[
                "config",
                "--local",
                "--replace-all",
                "remote.origin.url",
                remote,
            ],
            "",
            0,
        );
        runner.add(
            &[
                "config",
                "--local",
                "--replace-all",
                "remote.origin.pushurl",
                remote,
            ],
            "",
            0,
        );
        runner.add(&["rev-parse", "HEAD"], "0123456789abcdef\n", 0);
        runner.add(&["rev-parse", "--abbrev-ref", "HEAD"], branch, 0);
        runner.add(&["status", "--porcelain"], dirty, 0);
    };
    let untracked = std::path::Path::new(&binding.workspace_root).join("ordinary-file");
    std::fs::write(&untracked, "keep")?;
    assert!(matches!(
        call(true, remote).await,
        Err(crate::error::AppError::Conflict(_))
    ));
    assert_eq!(std::fs::read_to_string(&untracked)?, "keep");
    runner.done();
    std::fs::remove_file(untracked)?;
    runner.add(&["clone", "--depth", "1", remote, "."], "", 0);
    tail(remote, "", "main\n");
    let cloned = call(false, remote).await.unwrap().0;
    assert!(cloned.cloned && !cloned.dirty);
    for key in ["remote.origin.url", "remote.origin.pushurl"] {
        assert_eq!(runner.config.lock().unwrap()[key], vec![remote.to_string()]);
    }
    runner.done();
    std::fs::create_dir(binding.workspace_root.clone() + "/.git")?;
    runner.add(&["status", "--porcelain"], " M file\n", 0);
    assert!(matches!(
        call(false, remote).await,
        Err(crate::error::AppError::Conflict(_))
    ));
    runner.done();
    runner.add(&["status", "--porcelain"], " M file\n", 0);
    tail(remote, " M file\n", "HEAD\n");
    let preserved = call(true, remote).await.unwrap().0;
    assert!(preserved.dirty && !preserved.cloned && preserved.branch.is_none());
    runner.done();
    runner.add(&["status", "--porcelain"], "", 0);
    tail(remote, "", "main\n");
    let clean_preserved = call(true, remote).await.unwrap().0;
    assert!(!clean_preserved.dirty && clean_preserved.branch.as_deref() == Some("main"));
    runner.done();
    let work = std::path::Path::new(&binding.workspace_root).join("work.txt");
    let index = std::path::Path::new(&binding.workspace_root).join(".git/index");
    std::fs::write(&work, "uncommitted content")?;
    std::fs::write(&index, "staged content")?;
    for head in ["ahead-head", "diverged-head"] {
        runner.add(&["status", "--porcelain"], " M work.txt\n", 0);
        runner.add(
            &[
                "config",
                "--local",
                "--replace-all",
                "remote.origin.url",
                remote,
            ],
            "",
            0,
        );
        runner.add(
            &[
                "config",
                "--local",
                "--replace-all",
                "remote.origin.pushurl",
                remote,
            ],
            "",
            0,
        );
        runner.add(&["rev-parse", "HEAD"], head, 0);
        runner.add(&["rev-parse", "--abbrev-ref", "HEAD"], "main", 0);
        runner.add(&["status", "--porcelain"], " M work.txt\n", 0);
        let response = call(true, remote).await.unwrap().0;
        assert_eq!(response.head_commit.as_deref(), Some(head));
        assert_eq!(std::fs::read_to_string(&work)?, "uncommitted content");
        assert_eq!(std::fs::read_to_string(&index)?, "staged content");
        runner.done();
    }
    let native = "https://native:legacy@example.invalid/repo";
    runner.add(&["status", "--porcelain"], "", 0);
    runner.add(&["branch", "--show-current"], "main\n", 0);
    runner.add(&["pull", "--ff-only", native, "main"], "", 0);
    tail(native, "", "main\n");
    let clean = call(false, native).await.unwrap().0;
    assert!(!clean.dirty && clean.branch.as_deref() == Some("main"));
    for key in ["remote.origin.url", "remote.origin.pushurl"] {
        assert_eq!(runner.config.lock().unwrap()[key], vec![native.to_string()]);
    }
    runner.done();
    runner.add(&["status", "--porcelain"], "bad status", 1);
    assert!(matches!(
        call(true, remote).await,
        Err(crate::error::AppError::Conflict(_))
    ));
    runner.done();
    runner.add(&["status", "--porcelain"], "", 0);
    runner.add(
        &[
            "config",
            "--local",
            "--replace-all",
            "remote.origin.url",
            remote,
        ],
        "error https://limited:token@example.invalid/repo",
        1,
    );
    let error = call(true, remote).await.unwrap_err();
    assert!(!format!("{error:?}").contains("limited:token"));
    runner.done();
    for (head, head_code, branch, branch_code) in [
        ("", 0, "main", 0),
        ("bad", 1, "main", 0),
        ("012345", 0, "", 1),
    ] {
        runner.add(&["status", "--porcelain"], "", 0);
        runner.add(
            &[
                "config",
                "--local",
                "--replace-all",
                "remote.origin.url",
                remote,
            ],
            "",
            0,
        );
        runner.add(
            &[
                "config",
                "--local",
                "--replace-all",
                "remote.origin.pushurl",
                remote,
            ],
            "",
            0,
        );
        runner.add(&["rev-parse", "HEAD"], head, head_code);
        runner.add(&["rev-parse", "--abbrev-ref", "HEAD"], branch, branch_code);
        assert!(matches!(
            call(true, remote).await,
            Err(crate::error::AppError::Conflict(_))
        ));
        runner.done();
    }
    let expected = runner_protocol::workspace_principal::application_principal_owner(
        817301,
        binding.workspace_binding_id,
        "principal",
        "code_project:one",
    )
    .unwrap();
    assert!(
        runner
            .owners
            .lock()
            .unwrap()
            .iter()
            .all(|owner| owner == &expected)
    );
    sqlx::raw_sql(include_str!(
        "../../sql/principal_admission/test_coverage_seed.sql"
    ))
    .execute(&pool)
    .await?;
    sqlx::query(include_str!(
        "../../sql/principal_admission/test_p4m_environment.sql"
    ))
    .execute(&pool)
    .await?;
    sqlx::query(include_str!(
        "../../sql/principal_admission/test_p4m_native.sql"
    ))
    .bind(binding.workspace_binding_id)
    .execute(&pool)
    .await?;
    assert!(call(true, remote).await.is_err());
    runner.done();
    sqlx::query(include_str!(
        "../../sql/principal_admission/test_p4m_native_delete.sql"
    ))
    .execute(&pool)
    .await?;
    queries::release_workspace_write_lease(&pool, binding.workspace_binding_id, "principal")
        .await?
        .unwrap();
    assert!(call(true, remote).await.is_err());
    runner.done();
    pool.close().await;
    Ok(())
}

#[test]
fn git_sync_fenced_gate_denies_displaced_owner_after_handover() {
    let now = Utc::now();
    let fresh = WorkspaceBindingResponse {
        workspace_binding_id: 7,
        application_id: 1,
        external_user_id: "__resource__".into(),
        workspace_key: "code_project:abc".into(),
        external_user_hash: "hash".into(),
        workspace_root: "/tmp/ws".into(),
        is_active: true,
        last_used_at: now,
        created_at: now,
        updated_at: now,
        lifecycle_state: "active".into(),
        archived_at: None,
        resource_kind: Some("code_project".into()),
        resource_id: Some("abc".into()),
        write_lease_owner: Some("conversation:2".into()),
        write_lease_acquired_at: Some(now),
        write_lease_expires_at: Some(now + Duration::minutes(10)),
    };
    assert!(crate::actor::admit_fenced_write(Some(&fresh), Some("conversation:1")).is_err());
    assert!(crate::actor::admit_fenced_write(Some(&fresh), Some("conversation:2")).is_ok());
}
