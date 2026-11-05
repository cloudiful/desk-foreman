//! Owned fixtures for the durable Git-job suite.
//!
//! Each fixture creates one application and one runner manager plus any number
//! of code-project bindings. Cleanup removes exactly those owned rows in one
//! statement, so a failed cleanup is reported instead of silently leaving rows
//! behind. No fixture stores a remote URL, credential, or caller input.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use desk_foreman::AppState;
use desk_foreman::config::{AppConfig, GitJobSettings};
use desk_foreman::db::workspace_git_jobs::types::{AdmissionLimits, NewGitJob};
use desk_foreman::policy::{ALL_SCOPES, ResourceLimits};
use desk_foreman::runner::{RunnerFuture, RunnerService};
use runner_protocol::workspace_principal::application_principal_owner;
use runner_protocol::{
    CancelSessionRequest, CommandOutput, ExecRequest, InputRequest, RunnerCommandRequest,
    RunnerOwner, RunnerSessionStatus, ShellToolOutput,
};
use sqlx::PgPool;
use uuid::Uuid;

const INSERT_APPLICATION: &str = include_str!("fixtures/insert_application.sql");
const INSERT_MANAGER: &str = include_str!("fixtures/insert_runner_manager.sql");
const INSERT_BINDING: &str = include_str!("fixtures/insert_binding.sql");
const INSERT_GIT_JOB: &str = include_str!("fixtures/insert_git_job.sql");
const SET_JOB_CLAIMED: &str = include_str!("fixtures/set_job_claimed.sql");
const SET_JOB_DEADLINE: &str = include_str!("fixtures/set_job_deadline.sql");
const READ_STATE: &str = include_str!("fixtures/read_fixture_state.sql");
const INSERT_NATIVE_TASK: &str =
    include_str!("../../src/sql/execution_environments/tasks/insert_task.sql");

#[derive(Clone)]
pub struct Binding {
    pub binding_id: i64,
    pub code_project_id: Uuid,
    pub workspace_key: String,
    pub lease_acquired_at: DateTime<Utc>,
}

fn micros(value: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(value.timestamp_micros())
        .expect("microsecond timestamp is representable")
}

async fn insert_manager(pool: &PgPool, name: &str) -> (i64, String) {
    let token = format!("git-manager-token-{}", Uuid::new_v4().simple());
    let manager_id: i64 = sqlx::query_scalar(INSERT_MANAGER)
        .bind(name)
        .bind(format!("http://{name}.invalid"))
        .bind(desk_foreman::auth::hash_bearer_token(&token))
        .fetch_one(pool)
        .await
        .expect("insert fixture runner manager");
    (manager_id, token)
}

pub struct Fixture {
    pub application_id: i64,
    pub manager_id: i64,
    pub manager_token: String,
    pub lease_owner: String,
    pub lease_acquired_at: DateTime<Utc>,
    pub lease_expires_at: DateTime<Utc>,
    pub external_user_id: String,
    pub bindings: Vec<Binding>,
    manager_ids: Vec<i64>,
    workspace_root: PathBuf,
    _temp: tempfile::TempDir,
}

#[derive(sqlx::FromRow)]
pub struct JobState {
    pub token: String,
    pub state: String,
    pub cleanup_confirmed: bool,
    pub claimed_at: Option<DateTime<Utc>>,
    pub terminal_at: Option<DateTime<Utc>>,
    pub reserved_bytes: i64,
    pub principal_digest: String,
}

#[derive(sqlx::FromRow)]
pub struct Usage {
    pub global_count: i64,
    pub global_bytes: i64,
    pub binding_count: i64,
    pub manager_count: i64,
    pub manager_bytes: i64,
}

impl Usage {
    /// Limits with `extra` free slots above the retained rows already counted
    /// in this issue database. The suite never truncates the retained
    /// database, so caps are asserted relative to its current usage.
    pub fn generous(&self, extra: i64) -> AdmissionLimits {
        AdmissionLimits {
            max_active: self.global_count + extra,
            max_bytes: i64::MAX,
            max_per_binding: (self.binding_count + extra).max(1),
            max_per_manager: (self.manager_count + extra).max(1),
            max_bytes_per_manager: i64::MAX,
        }
    }

    pub fn global_count(&self, extra: i64) -> AdmissionLimits {
        AdmissionLimits {
            max_active: self.global_count + extra,
            ..self.generous(i64::MAX / 4)
        }
    }

    pub fn global_bytes(&self, extra: i64) -> AdmissionLimits {
        AdmissionLimits {
            max_bytes: self.global_bytes.saturating_add(extra),
            ..self.generous(i64::MAX / 4)
        }
    }

    pub fn manager_bytes(&self, extra: i64) -> AdmissionLimits {
        AdmissionLimits {
            max_bytes_per_manager: self.manager_bytes.saturating_add(extra),
            ..self.generous(i64::MAX / 4)
        }
    }
}

pub struct SeedJob<'a> {
    pub binding: &'a Binding,
    pub state: &'a str,
    pub reserved_bytes: i64,
    pub deadline_at: DateTime<Utc>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub cleanup_confirmed: bool,
    pub terminal_at: Option<DateTime<Utc>>,
}

impl Fixture {
    pub async fn setup(pool: &PgPool) -> Self {
        let temp = tempfile::tempdir().expect("fixture tempdir");
        let workspace_root = temp.path().join("workspaces");
        std::fs::create_dir_all(&workspace_root).expect("fixture workspace root");
        let suffix = Uuid::new_v4().simple().to_string();
        let application_id: i64 = sqlx::query_scalar(INSERT_APPLICATION)
            .bind(format!("git-fixture-{suffix}"))
            .fetch_one(pool)
            .await
            .expect("insert fixture application");
        let (manager_id, manager_token) =
            insert_manager(pool, &format!("git-manager-{suffix}")).await;
        let now = micros(Utc::now());
        let mut fixture = Self {
            application_id,
            manager_id,
            manager_token,
            manager_ids: vec![manager_id],
            lease_owner: format!("git-owner-{suffix}"),
            lease_acquired_at: now,
            lease_expires_at: now + ChronoDuration::minutes(10),
            external_user_id: format!("git-user-{suffix}"),
            bindings: Vec::new(),
            workspace_root,
            _temp: temp,
        };
        fixture
            .add_binding(pool, &format!("code_project:{suffix}"))
            .await;
        fixture
    }

    pub async fn add_manager(&mut self, pool: &PgPool) -> (i64, String) {
        let suffix = Uuid::new_v4().simple().to_string();
        let (manager_id, token) = insert_manager(pool, &format!("git-manager-{suffix}")).await;
        self.manager_ids.push(manager_id);
        (manager_id, token)
    }

    pub async fn add_binding(&mut self, pool: &PgPool, workspace_key: &str) -> Binding {
        self.add_binding_with_lease(
            pool,
            workspace_key,
            self.lease_owner.clone(),
            self.lease_acquired_at,
            self.lease_expires_at,
        )
        .await
    }

    pub async fn add_binding_with_lease(
        &mut self,
        pool: &PgPool,
        workspace_key: &str,
        lease_owner: String,
        acquired_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Binding {
        let code_project_id = Uuid::new_v4();
        let resource_id = code_project_id.to_string();
        let acquired_at = micros(acquired_at);
        let expires_at = micros(expires_at);
        let workspace_root = self
            .workspace_root
            .join(format!("binding-{}", self.bindings.len()));
        std::fs::create_dir_all(&workspace_root).expect("binding workspace");
        let binding_id: i64 = sqlx::query_scalar(INSERT_BINDING)
            .bind(self.application_id)
            .bind(&self.external_user_id)
            .bind(workspace_key)
            .bind(format!("hash-{}", Uuid::new_v4().simple()))
            .bind(workspace_root.to_string_lossy().to_string())
            .bind("code_project")
            .bind(&resource_id)
            .bind(&lease_owner)
            .bind(acquired_at)
            .bind(expires_at)
            .fetch_one(pool)
            .await
            .expect("insert fixture binding");
        let binding = Binding {
            binding_id,
            code_project_id,
            workspace_key: workspace_key.to_string(),
            lease_acquired_at: acquired_at,
        };
        self.bindings.push(binding.clone());
        binding
    }

    pub fn workspace_root(&self) -> &std::path::Path {
        &self.workspace_root
    }

    pub async fn add_derived_binding(&mut self, pool: &PgPool) -> Binding {
        let code_project_id = Uuid::new_v4();
        let resource_id = code_project_id.to_string();
        let workspace_key = format!("code_project:{resource_id}");
        let hash = desk_foreman::workspace::resource_workspace_hash("code_project", &resource_id);
        let workspace_root = self
            .workspace_root
            .join("apps")
            .join(self.application_id.to_string())
            .join("resources")
            .join(hash);
        std::fs::create_dir_all(&workspace_root).expect("derived binding workspace");
        let binding_id: i64 = sqlx::query_scalar(INSERT_BINDING)
            .bind(self.application_id)
            .bind(&self.external_user_id)
            .bind(&workspace_key)
            .bind(format!("hash-{}", Uuid::new_v4().simple()))
            .bind(workspace_root.to_string_lossy().to_string())
            .bind("code_project")
            .bind(&resource_id)
            .bind(&self.lease_owner)
            .bind(self.lease_acquired_at)
            .bind(self.lease_expires_at)
            .fetch_one(pool)
            .await
            .expect("insert derived fixture binding");
        let binding = Binding {
            binding_id,
            code_project_id,
            workspace_key,
            lease_acquired_at: self.lease_acquired_at,
        };
        self.bindings.push(binding.clone());
        binding
    }

    pub async fn seed_observation(
        &self,
        pool: &PgPool,
        binding: &Binding,
        status: &str,
    ) -> anyhow::Result<()> {
        self.seed_observation_for(pool, binding, status, self.manager_id)
            .await
    }

    pub async fn seed_observation_for(
        &self,
        pool: &PgPool,
        binding: &Binding,
        status: &str,
        manager_id: i64,
    ) -> anyhow::Result<()> {
        desk_foreman::db::queries::touch_runner_manager(pool, manager_id).await?;
        let owner = application_principal_owner(
            self.application_id,
            binding.binding_id,
            &self.external_user_id,
            &binding.workspace_key,
        )
        .map_err(|error| anyhow::anyhow!(error))?;
        let root =
            desk_foreman::db::queries::find_workspace_binding_by_id(pool, binding.binding_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("fixture binding is absent"))?
                .workspace_root;
        desk_foreman::db::queries::save_workspace_runner(
            pool,
            desk_foreman::db::workspace_runners::SaveWorkspaceRunner {
                runner_manager_id: manager_id,
                owner: &owner,
                container_name: &format!(
                    "git-fixture-container-{}-{manager_id}",
                    binding.binding_id
                ),
                container_id: Some("fixture-container"),
                runtime: "docker",
                runtime_class: None,
                image_name: "fixture-helper",
                status,
                network_enabled: true,
                workspace_root: &root,
                last_error: None,
            },
        )
        .await?;
        Ok(())
    }

    pub fn new_job(&self, binding: &Binding, token: &str, reserved_bytes: i64) -> NewGitJob {
        NewGitJob {
            job_id: Uuid::new_v4(),
            token: token.to_string(),
            operation_id: Uuid::new_v4(),
            application_id: self.application_id,
            workspace_binding_id: binding.binding_id,
            assigned_manager_id: self.manager_id,
            code_project_id: binding.code_project_id,
            authorization_generation: Uuid::new_v4(),
            canonical_branch: "main".to_string(),
            expected_base: None,
            lease_owner: self.lease_owner.clone(),
            lease_acquired_at: binding.lease_acquired_at,
            reserved_bytes,
            deadline_at: Utc::now() + ChronoDuration::minutes(5),
        }
    }

    pub async fn usage(pool: &PgPool, binding_id: i64, manager_id: i64) -> Usage {
        sqlx::query_as(include_str!(
            "../../src/sql/workspace_git_jobs/active_git_job_usage.sql"
        ))
        .bind(binding_id)
        .bind(manager_id)
        .fetch_one(pool)
        .await
        .expect("read current git job usage")
    }

    pub async fn seed_native_task(&self, pool: &PgPool, binding: &Binding) -> Uuid {
        use desk_foreman::db::queries::{EnsureOutcome, ensure_execution_environment};

        let environment_id = match ensure_execution_environment(
            pool,
            self.application_id,
            &self.external_user_id,
            "opencode",
        )
        .await
        .expect("ensure native environment")
        {
            EnsureOutcome::Created(row) | EnsureOutcome::Existing(row) => row.environment_id,
            other => panic!("unexpected environment outcome: {other:?}"),
        };
        let task_id = Uuid::new_v4();
        sqlx::query(INSERT_NATIVE_TASK)
            .bind(task_id)
            .bind(format!("native-{}", Uuid::new_v4().simple()))
            .bind(environment_id)
            .bind(self.application_id)
            .bind(binding.binding_id)
            .bind("native")
            .bind(0_i64)
            .bind(self.manager_id)
            .fetch_optional(pool)
            .await
            .expect("insert native task");
        task_id
    }

    pub async fn seed_git_job(&self, pool: &PgPool, seed: SeedJob<'_>) -> Uuid {
        let job_id = Uuid::new_v4();
        sqlx::query(INSERT_GIT_JOB)
            .bind(job_id)
            .bind(format!("job-{}", job_id.simple()))
            .bind(Uuid::new_v4())
            .bind(self.application_id)
            .bind(seed.binding.binding_id)
            .bind(self.manager_id)
            .bind(seed.binding.code_project_id)
            .bind(Uuid::new_v4())
            .bind(seed.state)
            .bind("main")
            .bind(Option::<String>::None)
            .bind(self.principal_digest_for(seed.binding))
            .bind(&self.lease_owner)
            .bind(seed.binding.lease_acquired_at)
            .bind(seed.reserved_bytes)
            .bind(seed.claimed_at)
            .bind(seed.deadline_at)
            .bind(seed.cleanup_confirmed)
            .bind(seed.terminal_at)
            .fetch_one(pool)
            .await
            .expect("insert seeded git job");
        job_id
    }

    pub async fn seed_git_job_with_expected_base(
        &self,
        pool: &PgPool,
        seed: SeedJob<'_>,
        expected_base: &str,
    ) -> Uuid {
        let job_id = Uuid::new_v4();
        sqlx::query(INSERT_GIT_JOB)
            .bind(job_id)
            .bind(format!("job-{}", job_id.simple()))
            .bind(Uuid::new_v4())
            .bind(self.application_id)
            .bind(seed.binding.binding_id)
            .bind(self.manager_id)
            .bind(seed.binding.code_project_id)
            .bind(Uuid::new_v4())
            .bind(seed.state)
            .bind("main")
            .bind(Some(expected_base.to_owned()))
            .bind(self.principal_digest_for(seed.binding))
            .bind(&self.lease_owner)
            .bind(seed.binding.lease_acquired_at)
            .bind(seed.reserved_bytes)
            .bind(seed.claimed_at)
            .bind(seed.deadline_at)
            .bind(seed.cleanup_confirmed)
            .bind(seed.terminal_at)
            .fetch_one(pool)
            .await
            .expect("insert seeded git job");
        job_id
    }

    pub fn principal_digest_for(&self, binding: &Binding) -> String {
        match application_principal_owner(
            self.application_id,
            binding.binding_id,
            &self.external_user_id,
            &binding.workspace_key,
        )
        .expect("derive principal")
        {
            RunnerOwner::ApplicationPrincipal {
                principal_digest, ..
            } => principal_digest,
            other => panic!("unexpected owner: {other:?}"),
        }
    }

    pub async fn read_state(pool: &PgPool, job_id: Uuid) -> JobState {
        sqlx::query_as(READ_STATE)
            .bind(job_id)
            .fetch_one(pool)
            .await
            .expect("read fixture job state")
    }

    pub async fn mark_claimed(pool: &PgPool, job_id: Uuid) {
        sqlx::query(SET_JOB_CLAIMED)
            .bind(job_id)
            .fetch_optional(pool)
            .await
            .expect("mark job claimed");
    }

    pub async fn set_deadline(pool: &PgPool, job_id: Uuid, deadline: DateTime<Utc>) {
        sqlx::query(SET_JOB_DEADLINE)
            .bind(job_id)
            .bind(deadline)
            .fetch_optional(pool)
            .await
            .expect("set job deadline");
    }

    /// Archive a binding and wait until its database timestamp is strictly in
    /// the past for this process, so a zero-retention janitor sweep selects it
    /// despite small host/database clock skew.
    pub async fn archive_for_janitor(pool: &PgPool, binding_id: i64) {
        desk_foreman::db::queries::set_workspace_binding_state(pool, binding_id, "archived")
            .await
            .expect("archive binding for janitor");
        for _ in 0..100 {
            let archived_at =
                desk_foreman::db::queries::find_workspace_binding_by_id(pool, binding_id)
                    .await
                    .expect("read archived binding")
                    .and_then(|binding| binding.archived_at);
            if archived_at.is_some_and(|archived_at| archived_at < Utc::now()) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("archived binding never became eligible for the janitor");
    }

    pub async fn cleanup(self, pool: &PgPool) {
        for manager_id in &self.manager_ids {
            sqlx::query_file!(
                "tests/workspace_git_jobs/fixtures/delete_fixture.sql",
                self.application_id,
                manager_id
            )
            .execute(pool)
            .await
            .expect("fixture cleanup must remove every owned row");
        }
    }
}

pub fn credential_identity(
    row: &desk_foreman::db::workspace_git_jobs::types::GitJobRecord,
) -> desk_foreman::runner::git_job_credentials::GitJobCredentialIdentity {
    desk_foreman::runner::git_job_credentials::GitJobCredentialIdentity {
        job_id: row.job_id,
        operation_id: row.operation_id,
        assigned_manager_id: row.assigned_manager_id,
        application_id: row.application_id,
        workspace_binding_id: row.workspace_binding_id,
        principal_digest: row.principal_digest.clone(),
        code_project_id: row.code_project_id,
        authorization_generation: row.authorization_generation,
        canonical_branch: row.canonical_branch.clone(),
        deadline_unix_ms: row.deadline_at.timestamp_millis(),
    }
}

pub fn synthetic_credential(
    code_project_id: Uuid,
    deadline_unix_ms: i64,
    mac: char,
) -> runner_protocol::GitJobCredential {
    runner_protocol::GitJobCredential {
        username: runner_protocol::GIT_JOB_CREDENTIAL_USERNAME.to_string(),
        password: format!("smcpw1.{}.{}", Uuid::new_v4(), mac.to_string().repeat(64)),
        remote_url: format!(
            "https://git.example.invalid/api/internal/code-projects/{code_project_id}/git"
        ),
        deadline_unix_ms,
    }
}

pub fn insert_credential(
    state: &AppState,
    row: &desk_foreman::db::workspace_git_jobs::types::GitJobRecord,
    credential: runner_protocol::GitJobCredential,
) {
    state
        .git_job_credentials
        .insert(
            desk_foreman::runner::git_job_credentials::GitJobCredentialEntry {
                identity: credential_identity(row),
                credential,
            },
        )
        .expect("insert fixture credential");
}

/// Runner that never provisions: archive cancellation sees no sessions.
pub struct NoopRunner;

impl RunnerService for NoopRunner {
    fn exec_shell<'a>(
        &'a self,
        _request: ExecRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async move { anyhow::bail!("noop runner: exec_shell unsupported") })
    }

    fn write_stdin<'a>(
        &'a self,
        _request: InputRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async move { anyhow::bail!("noop runner: write_stdin unsupported") })
    }

    fn cancel_session<'a>(
        &'a self,
        _request: CancelSessionRequest,
    ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>> {
        Box::pin(async move { anyhow::bail!("noop runner: cancel_session unsupported") })
    }

    fn list_sessions<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>> {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn run_command<'a>(
        &'a self,
        _request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
        Box::pin(async move { anyhow::bail!("noop runner: run_command unsupported") })
    }

    fn cleanup_runner_owner<'a>(
        &'a self,
        _owner: RunnerOwner,
    ) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

fn test_config(workspace_root: PathBuf, git_jobs: Option<GitJobSettings>) -> AppConfig {
    AppConfig {
        bind_addr: "127.0.0.1:0".to_string(),
        mcp_allowed_hosts: Vec::new(),
        workspace_root: workspace_root.clone(),
        default_shell: "bash".to_string(),
        session_idle_ttl: Duration::from_secs(60),
        max_output_bytes: 64 * 1024,
        server_scopes: ALL_SCOPES.iter().map(|scope| scope.to_string()).collect(),
        server_limits: ResourceLimits {
            max_timeout_ms: Some(600_000),
            max_output_bytes: Some(64 * 1024),
            max_file_bytes: Some(50 * 1024 * 1024),
            max_sessions: None,
            network_enabled: true,
        },
        workspace_retention: Duration::from_secs(0),
        git_jobs,
        database_url: "postgres://example.invalid/test".to_string(),
        web_session_ttl: Duration::from_secs(3600),
        web_cookie_name: "desk_foreman_session".to_string(),
        web_cookie_secure: false,
        bootstrap_admin_login: None,
        bootstrap_admin_password: None,
        bootstrap_admin_display_name: None,
        bootstrap_admin_email: None,
        bootstrap_admin_timezone: "UTC".to_string(),
        frontend_dist: workspace_root.join("frontend"),
        build_started_at: SystemTime::now(),
    }
}

pub struct AdminHarness {
    pub base: String,
    state: AppState,
    cookie: String,
    user_id: i64,
    workspace_root: PathBuf,
    handle: tokio::task::JoinHandle<()>,
}

impl AdminHarness {
    pub async fn setup(pool: PgPool) -> Self {
        let unique = Uuid::new_v4();
        let workspace_root =
            std::env::temp_dir().join(format!("desk-foreman-issue-1180-admin-{unique}"));
        std::fs::create_dir_all(&workspace_root).expect("admin workspace root");
        let config = test_config(workspace_root.clone(), None);
        let broker = desk_foreman::runner::RunnerBroker::new(pool.clone());
        let runner: Arc<dyn RunnerService> = Arc::new(NoopRunner);
        let state = AppState {
            config: Arc::new(config),
            runner,
            runner_broker: broker,
            git_job_credentials: Arc::new(
                desk_foreman::runner::git_job_credentials::GitJobCredentialStore::new(),
            ),
            db: pool,
        };
        let router = desk_foreman::api::router().with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind admin port");
        let addr = listener.local_addr().expect("admin local addr");

        let user_root = workspace_root.join("user");
        std::fs::create_dir_all(&user_root).expect("admin user workspace");
        let password = format!("g8e-password-{unique}");
        let password_hash = desk_foreman::auth::hash_password(&password).expect("hash password");
        let user = desk_foreman::db::queries::create_user(
            &state.db,
            &desk_foreman::db::types::CreateUserRequest {
                login_name: format!("g8e-admin-{unique}"),
                password,
                display_name: "G8e Admin".to_string(),
                email: format!("g8e-admin-{unique}@local.invalid"),
                timezone: "UTC".to_string(),
                workspace_root: Some(user_root.to_string_lossy().into_owned()),
                is_admin: true,
            },
            &password_hash,
        )
        .await
        .expect("create admin fixture user");
        let session_id = Uuid::new_v4();
        let expires_at = Utc::now()
            + ChronoDuration::from_std(state.config.web_session_ttl).expect("admin session ttl");
        desk_foreman::db::queries::create_session(&state.db, session_id, user.user_id, expires_at)
            .await
            .expect("create admin fixture session");
        let cookie = format!("{}={session_id}", state.config.web_cookie_name);
        let handle = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("serve admin router");
        });

        Self {
            base: format!("http://{addr}"),
            state,
            cookie,
            user_id: user.user_id,
            workspace_root,
            handle,
        }
    }

    pub async fn cleanup(&self, pool: &PgPool) {
        self.handle.abort();
        sqlx::query_file!(
            "tests/workspace_git_jobs/fixtures/delete_admin_harness.sql",
            self.user_id
        )
        .execute(pool)
        .await
        .expect("admin fixture cleanup must remove every owned row");
        let _ = std::fs::remove_dir_all(&self.workspace_root);
    }

    pub async fn archive(&self, binding_id: i64) -> u16 {
        post_json(
            &self.base,
            &format!("/api/admin/workspace-bindings/{binding_id}/archive"),
            Some(&self.cookie),
            b"",
        )
        .await
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }
}

impl Drop for AdminHarness {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

pub struct ManagerHarness {
    pub base: String,
    state: AppState,
    handle: tokio::task::JoinHandle<()>,
}

impl ManagerHarness {
    pub async fn setup(pool: PgPool) -> Self {
        let workspace_root = std::env::temp_dir().join("desk-foreman-issue-1180-manager-workspace");
        std::fs::create_dir_all(&workspace_root).expect("manager workspace root");
        let config = test_config(workspace_root.clone(), None);
        let broker = desk_foreman::runner::RunnerBroker::new(pool.clone());
        let runner: Arc<dyn RunnerService> = Arc::new(NoopRunner);
        let state = AppState {
            config: Arc::new(config),
            runner,
            runner_broker: broker,
            git_job_credentials: Arc::new(
                desk_foreman::runner::git_job_credentials::GitJobCredentialStore::new(),
            ),
            db: pool,
        };
        let router = desk_foreman::api::router().with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind manager port");
        let addr = listener.local_addr().expect("manager local addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("serve manager router");
        });
        Self {
            base: format!("http://{addr}"),
            state,
            handle,
        }
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    pub async fn get(
        &self,
        path: &str,
        bearer: Option<&str>,
        extra_headers: &[(&str, &str)],
    ) -> HttpResponse {
        send(&self.base, "GET", path, bearer, extra_headers, &[]).await
    }

    pub async fn post_json(&self, path: &str, bearer: Option<&str>, body: &[u8]) -> HttpResponse {
        send(&self.base, "POST", path, bearer, &[], body).await
    }
}

pub struct RouteHarness {
    pub base: String,
    handle: tokio::task::JoinHandle<()>,
}

impl RouteHarness {
    pub async fn setup(
        pool: PgPool,
        workspace_root: PathBuf,
        git_jobs: Option<GitJobSettings>,
        runner: Arc<dyn RunnerService>,
    ) -> Self {
        std::fs::create_dir_all(&workspace_root).expect("route workspace root");
        let state = AppState {
            config: Arc::new(test_config(workspace_root, git_jobs)),
            runner,
            runner_broker: desk_foreman::runner::RunnerBroker::new(pool.clone()),
            git_job_credentials: Arc::new(
                desk_foreman::runner::git_job_credentials::GitJobCredentialStore::new(),
            ),
            db: pool,
        };
        let router = desk_foreman::api::router().with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind route port");
        let addr = listener.local_addr().expect("route local addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("serve route router");
        });
        Self {
            base: format!("http://{addr}"),
            handle,
        }
    }

    pub async fn post_raw(
        &self,
        path: &str,
        bearer: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> HttpResponse {
        let headers: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        send(
            &self.base,
            "POST",
            path,
            Some(bearer),
            &headers,
            body.as_bytes(),
        )
        .await
    }
}

impl Drop for RouteHarness {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl Drop for ManagerHarness {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn post_json(base: &str, path: &str, cookie: Option<&str>, body: &[u8]) -> u16 {
    let extra: Vec<(&str, &str)> = cookie
        .map(|cookie| vec![("Cookie", cookie)])
        .unwrap_or_default();
    send(base, "POST", path, None, &extra, body).await.status
}

async fn send(
    base: &str,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    extra_headers: &[(&str, &str)],
    body: &[u8],
) -> HttpResponse {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let without_scheme = base.strip_prefix("http://").expect("http base URL");
    let (host, port) = without_scheme.rsplit_once(':').expect("host:port");
    let mut stream = tokio::net::TcpStream::connect(format!("{host}:{port}"))
        .await
        .expect("connect test server");
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(bearer) = bearer {
        head.push_str(&format!("Authorization: Bearer {bearer}\r\n"));
    }
    for (name, value) in extra_headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await.expect("write head");
    stream.write_all(body).await.expect("write body");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read response");
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse().ok())
        .expect("HTTP status line");
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .collect();
    HttpResponse {
        status,
        headers,
        body: body.trim().to_string(),
    }
}
