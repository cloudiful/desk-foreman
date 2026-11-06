//! Guarded janitor retention regression (issue 1180 J1).
//!
//! These ignored cases run only against the retained issue database named by
//! `DF_GIT_TEST_DATABASE_URL`. They create owned fixture rows through the
//! production APIs, drive the real janitor path, and always remove exactly the
//! rows they created. No database is created, dropped, or migrated.

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use futures_util::FutureExt;
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

use crate::AppState;
use crate::db;
use crate::db::audit::AuditLogEntry;
use crate::db::types::{
    CreateApplicationRequest, CreateRunnerManagerRequest, ListAuditLogsParams,
    WorkspaceBindingResponse,
};
use crate::db::workspace_git_jobs::lifecycle::{claim_next_git_job, complete_git_job};
use crate::db::workspace_git_jobs::store::admit_and_insert_git_job;
use crate::db::workspace_git_jobs::types::{
    AdmissionLimits, GitJobCompletion, GitJobResult, NewGitJob,
};
use crate::lease_test_support::{FakeRunnerService, fake_state};

const REQUIRED_HOST: &str = "unraid.cloud1ful.com";
const REQUIRED_PORT: u16 = 5444;
const REQUIRED_DATABASE: &str = "desk_foreman_issue_1180";
const OWNERSHIP_MARK: &str = "desk-foreman issue #1180";
const IDENTITY_SQL: &str = include_str!("../../tests/workspace_git_jobs/issue_db_identity.sql");

#[derive(sqlx::FromRow)]
struct IssueDbIdentity {
    database_name: String,
    server_version: String,
    ownership_comment: Option<String>,
}

struct RetentionLedger {
    pool: PgPool,
    application_id: i64,
    manager_id: i64,
    root: tempfile::TempDir,
    binding_ids: Mutex<Vec<String>>,
    locked_dirs: Mutex<Vec<PathBuf>>,
}

impl RetentionLedger {
    async fn new() -> Self {
        let pool = issue_pool().await;
        let application = db::queries::create_application(
            &pool,
            &CreateApplicationRequest {
                name: unique_name("retention-app"),
                workspace_template: None,
                default_shell: None,
                default_scopes: None,
                max_timeout_ms: None,
                max_output_bytes: None,
                max_file_bytes: None,
                max_sessions: None,
                network_enabled: Some(false),
            },
        )
        .await
        .expect("create retention fixture application");
        let (manager, _) = db::queries::create_runner_manager(
            &pool,
            &CreateRunnerManagerRequest {
                name: unique_name("retention-manager"),
                endpoint: "http://127.0.0.1:1".to_string(),
                access_token: None,
                image: "retention-test-image".to_string(),
                network_enabled: false,
                max_output_bytes: 1024,
                max_timeout_ms: 1_000,
                max_sessions: 1,
                pids_limit: 16,
                memory_limit: "64m".to_string(),
                cpu_limit: "0.1".to_string(),
                host_workspace_root: None,
            },
        )
        .await
        .expect("create retention fixture runner manager");
        db::queries::touch_runner_manager(&pool, manager.runner_manager_id)
            .await
            .expect("mark retention fixture manager online");
        Self {
            pool,
            application_id: application.application_id,
            manager_id: manager.runner_manager_id,
            root: tempfile::tempdir().expect("retention temp root"),
            binding_ids: Mutex::new(Vec::new()),
            locked_dirs: Mutex::new(Vec::new()),
        }
    }

    fn track_binding(&self, binding_id: i64) {
        self.binding_ids
            .lock()
            .expect("binding ledger")
            .push(binding_id.to_string());
    }

    fn track_locked_dir(&self, path: PathBuf) {
        self.locked_dirs
            .lock()
            .expect("locked directory ledger")
            .push(path);
    }

    async fn create_binding(&self, label: &str) -> (WorkspaceBindingResponse, PathBuf) {
        let external_user_id = unique_name(label);
        let workspace_key = format!("code_project:{}", Uuid::new_v4());
        let resource_id = workspace_key
            .strip_prefix("code_project:")
            .expect("resource key")
            .to_string();
        let root = self.root.path().join(format!("binding-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("fixture workspace root");
        std::fs::write(root.join("keep"), "keep").expect("fixture workspace file");
        let binding = db::queries::create_workspace_binding(
            &self.pool,
            self.application_id,
            &external_user_id,
            &workspace_key,
            root.to_str().expect("fixture root path"),
            Some("code_project"),
            Some(&resource_id),
        )
        .await
        .expect("create retention fixture binding");
        self.track_binding(binding.workspace_binding_id);
        (binding, root)
    }

    async fn archive(&self, binding_id: i64) {
        db::queries::set_workspace_binding_state(&self.pool, binding_id, "archived")
            .await
            .expect("archive retention fixture binding")
            .expect("retention fixture binding exists");
    }

    async fn finish(&self) -> anyhow::Result<()> {
        for path in self
            .locked_dirs
            .lock()
            .expect("locked directory ledger")
            .iter()
        {
            restore_directory_permissions(path);
        }
        let binding_ids = self.binding_ids.lock().expect("binding ledger").clone();
        sqlx::query_file!(
            "src/sql/lifecycle/test_retention_cleanup.sql",
            self.application_id,
            self.manager_id,
            &binding_ids,
        )
        .execute(&self.pool)
        .await?;
        self.pool.close().await;
        Ok(())
    }
}

async fn run_guarded<F>(ledger: &RetentionLedger, body: F)
where
    F: std::future::Future<Output = ()>,
{
    let outcome = AssertUnwindSafe(body).catch_unwind().await;
    let cleanup = ledger.finish().await;
    if let Err(error) = cleanup {
        panic!("retention fixture cleanup failed: {error}");
    }
    if let Err(payload) = outcome {
        std::panic::resume_unwind(payload);
    }
}

async fn issue_pool() -> PgPool {
    let url = std::env::var("DF_GIT_TEST_DATABASE_URL").expect(
        "DF_GIT_TEST_DATABASE_URL is required for the retention suite; set it to the \
         task-owned issue database URL",
    );
    let options = PgConnectOptions::from_str(&url)
        .unwrap_or_else(|_| panic!("DF_GIT_TEST_DATABASE_URL is not a valid PostgreSQL URL"));
    assert_eq!(options.get_host(), REQUIRED_HOST);
    assert_eq!(options.get_port(), REQUIRED_PORT);
    assert_eq!(options.get_database(), Some(REQUIRED_DATABASE));
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(30))
        .connect_with(options)
        .await
        .expect("connect to the task-owned issue database");
    let identity: IssueDbIdentity = sqlx::query_as(IDENTITY_SQL)
        .fetch_one(&pool)
        .await
        .expect("read issue database identity");
    assert_eq!(identity.database_name, REQUIRED_DATABASE);
    assert!(identity.server_version.starts_with("18."));
    assert!(
        identity
            .ownership_comment
            .as_deref()
            .is_some_and(|comment| comment.contains(OWNERSHIP_MARK))
    );
    pool
}

fn unique_name(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}

fn state_with_root(ledger: &RetentionLedger) -> AppState {
    let mut state = fake_state(FakeRunnerService::new());
    state.db = ledger.pool.clone();
    let mut config = (*state.config).clone();
    config.workspace_root = ledger.root.path().to_path_buf();
    config.workspace_retention = Duration::from_millis(1);
    state.config = Arc::new(config);
    state
}

async fn success_audit_count(ledger: &RetentionLedger, binding_id: i64) -> usize {
    let page = db::queries::list_audit_logs(
        &ledger.pool,
        &ListAuditLogsParams {
            limit: Some(200),
            offset: Some(0),
            from: None,
            to: None,
            action: Some("workspace.delete".to_string()),
            search: None,
            status: None,
            actor_user_id: None,
            actor_application_id: None,
            workspace_binding_id: None,
            session_id: None,
            request_id: None,
        },
    )
    .await
    .expect("list janitor audits");
    page.items
        .iter()
        .filter(|entry| {
            entry.target_type == "workspace_binding"
                && entry.target_id == binding_id.to_string()
                && entry.status.as_deref() == Some("success")
        })
        .count()
}

async fn assert_retained(ledger: &RetentionLedger, binding_id: i64, root: &std::path::Path) {
    assert!(
        db::queries::find_workspace_binding_by_id(&ledger.pool, binding_id)
            .await
            .expect("read retained binding")
            .is_some(),
        "binding with retained references must survive the janitor"
    );
    assert!(root.exists(), "retained binding directory must survive");
    assert_eq!(
        success_audit_count(ledger, binding_id).await,
        0,
        "a retained binding must not get a delete-success audit"
    );
}

fn restore_directory_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
}

#[tokio::test]
#[ignore = "requires DF_GIT_TEST_DATABASE_URL (issue 1180 database)"]
async fn audit_reference_retains_archived_binding_and_directory() {
    let ledger = RetentionLedger::new().await;
    run_guarded(&ledger, async {
        let (binding, root) = ledger.create_binding("audit-ref").await;
        db::queries::record_audit(
            &ledger.pool,
            AuditLogEntry {
                actor_user_id: None,
                actor_application_id: None,
                actor_type: "system",
                action: "workspace.retention.fixture",
                target_type: "workspace_binding",
                target_id: &binding.workspace_binding_id.to_string(),
                workspace_binding_id: Some(binding.workspace_binding_id),
                external_user_id: Some(&binding.external_user_id),
                payload: serde_json::json!({}),
                request_id: None,
                session_id: None,
                duration_ms: None,
                status: Some("success"),
            },
        )
        .await
        .expect("record retained audit evidence");
        ledger.archive(binding.workspace_binding_id).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let state = state_with_root(&ledger);
        super::cleanup_archived_workspaces(&state)
            .await
            .expect("janitor sweep");
        assert_retained(&ledger, binding.workspace_binding_id, &root).await;
    })
    .await;
}

#[tokio::test]
#[ignore = "requires DF_GIT_TEST_DATABASE_URL (issue 1180 database)"]
async fn released_task_retains_archived_binding_and_directory() {
    let ledger = RetentionLedger::new().await;
    run_guarded(&ledger, async {
        let (binding, root) = ledger.create_binding("task-ref").await;
        let environment = match db::queries::ensure_execution_environment(
            &ledger.pool,
            ledger.application_id,
            &binding.external_user_id,
            "docker",
        )
        .await
        .expect("ensure fixture environment")
        {
            db::execution_environments::store::EnsureOutcome::Created(environment)
            | db::execution_environments::store::EnsureOutcome::Existing(environment) => {
                environment
            }
            other => panic!("unexpected ensure outcome: {other:?}"),
        };
        let task = db::execution_environments::tasks::admit_native_task(
            &ledger.pool,
            ledger.application_id,
            environment.environment_id,
            db::execution_environments::tasks::NativeTaskRequest {
                task_id: Uuid::new_v4(),
                workspace_binding_id: binding.workspace_binding_id,
                subject_external_user_id: binding.external_user_id.clone(),
                expected_generation: environment.generation,
                operation_token: unique_name("retention-task"),
            },
        )
        .await
        .expect("admit fixture native task");
        db::execution_environments::tasks::cancel_native_task(
            &ledger.pool,
            ledger.application_id,
            environment.environment_id,
            task.task_id,
            task.target_generation,
        )
        .await
        .expect("release fixture native task");
        ledger.archive(binding.workspace_binding_id).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let state = state_with_root(&ledger);
        super::cleanup_archived_workspaces(&state)
            .await
            .expect("janitor sweep");
        assert_retained(&ledger, binding.workspace_binding_id, &root).await;
    })
    .await;
}

#[tokio::test]
#[ignore = "requires DF_GIT_TEST_DATABASE_URL (issue 1180 database)"]
async fn terminal_git_job_retains_archived_binding_and_directory() {
    let ledger = RetentionLedger::new().await;
    run_guarded(&ledger, async {
        let (binding, root) = ledger.create_binding("git-ref").await;
        let lease_owner = unique_name("retention-lease");
        let leased = db::queries::acquire_workspace_write_lease(
            &ledger.pool,
            binding.workspace_binding_id,
            &lease_owner,
            600,
        )
        .await
        .expect("acquire fixture lease")
        .expect("fixture binding accepts the lease");
        let job = admit_and_insert_git_job(
            &ledger.pool,
            NewGitJob {
                job_id: Uuid::new_v4(),
                token: unique_name("retention-job"),
                operation_id: Uuid::new_v4(),
                application_id: ledger.application_id,
                workspace_binding_id: binding.workspace_binding_id,
                assigned_manager_id: ledger.manager_id,
                code_project_id: Uuid::parse_str(
                    binding.resource_id.as_deref().expect("fixture resource id"),
                )
                .expect("fixture resource UUID"),
                authorization_generation: Uuid::new_v4(),
                canonical_branch: "main".to_string(),
                expected_base: None,
                lease_owner: lease_owner.clone(),
                lease_acquired_at: leased
                    .write_lease_acquired_at
                    .expect("fixture lease timestamp"),
                reserved_bytes: 1,
                deadline_at: Utc::now() + ChronoDuration::seconds(60),
            },
            AdmissionLimits {
                max_active: 10,
                max_bytes: 1024,
                max_per_binding: 10,
                max_per_manager: 10,
                max_bytes_per_manager: 1024,
            },
        )
        .await
        .expect("admit fixture git job");
        claim_next_git_job(&ledger.pool, ledger.manager_id)
            .await
            .expect("claim fixture git job")
            .expect("fixture git job is claimable");
        complete_git_job(
            &ledger.pool,
            job.job_id,
            &job.token,
            ledger.manager_id,
            GitJobCompletion::Succeeded {
                result: GitJobResult::SyncSucceeded {
                    head: "a".repeat(40),
                    base: None,
                    published: true,
                },
            },
        )
        .await
        .expect("complete fixture git job");
        ledger.archive(binding.workspace_binding_id).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let state = state_with_root(&ledger);
        super::cleanup_archived_workspaces(&state)
            .await
            .expect("janitor sweep");
        assert_retained(&ledger, binding.workspace_binding_id, &root).await;
    })
    .await;
}

#[tokio::test]
#[ignore = "requires DF_GIT_TEST_DATABASE_URL (issue 1180 database)"]
async fn filesystem_failure_retains_first_binding_and_still_cleans_later_orphan() {
    use std::os::unix::fs::PermissionsExt;
    let ledger = RetentionLedger::new().await;
    run_guarded(&ledger, async {
        let (failing, failing_root) = ledger.create_binding("fs-failure").await;
        let (orphan, orphan_root) = ledger.create_binding("fs-orphan").await;
        ledger.archive(failing.workspace_binding_id).await;
        tokio::time::sleep(Duration::from_millis(2)).await;
        ledger.archive(orphan.workspace_binding_id).await;
        ledger.track_locked_dir(failing_root.clone());
        std::fs::set_permissions(&failing_root, std::fs::Permissions::from_mode(0o500))
            .expect("lock failing directory");
        tokio::time::sleep(Duration::from_millis(20)).await;
        let state = state_with_root(&ledger);
        super::cleanup_archived_workspaces(&state)
            .await
            .expect("janitor sweep continues after a per-binding failure");
        assert_retained(&ledger, failing.workspace_binding_id, &failing_root).await;
        assert!(
            db::queries::find_workspace_binding_by_id(&ledger.pool, orphan.workspace_binding_id)
                .await
                .expect("read orphan binding")
                .is_none(),
            "an eligible orphan after a failed binding must still be deleted"
        );
        assert!(!orphan_root.exists(), "orphan directory must be removed");
        assert_eq!(
            success_audit_count(&ledger, orphan.workspace_binding_id).await,
            1,
            "only the deleted orphan gets a success audit"
        );
        restore_directory_permissions(&failing_root);
    })
    .await;
}

#[tokio::test]
#[ignore = "requires DF_GIT_TEST_DATABASE_URL (issue 1180 database)"]
async fn no_reference_orphan_is_deleted_with_one_success_audit() {
    let ledger = RetentionLedger::new().await;
    run_guarded(&ledger, async {
        let (orphan, root) = ledger.create_binding("orphan").await;
        ledger.archive(orphan.workspace_binding_id).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let state = state_with_root(&ledger);
        super::cleanup_archived_workspaces(&state)
            .await
            .expect("janitor sweep");
        assert!(
            db::queries::find_workspace_binding_by_id(&ledger.pool, orphan.workspace_binding_id)
                .await
                .expect("read orphan binding")
                .is_none(),
            "an unreferenced archived binding must be deleted"
        );
        assert!(!root.exists(), "orphan directory must be removed");
        assert_eq!(
            success_audit_count(&ledger, orphan.workspace_binding_id).await,
            1
        );
    })
    .await;
}
