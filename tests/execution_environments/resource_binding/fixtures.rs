//! Fixtures for the lease-free resource binding cases (issue 815 P1).
//!
//! Split from `resource_binding_cases.rs` so both files stay inside the size
//! budget: this one owns the isolated database, application identity, runner
//! manager and admission setup, while the case file owns the assertions.
//!
//! Every fixture drives the real internal HTTP router and the real admission
//! path, so no case needs a raw SQL statement, Docker or a shared database.

use std::sync::Arc;

use serde_json::Value;
use uuid::Uuid;

use desk_foreman::db::{
    execution_environments::{
        EnsureOutcome, NativeTask, NativeTaskError, NativeTaskRequest, admit_native_task,
        ensure_execution_environment,
    },
    queries,
    types::{CreateRunnerManagerRequest, UpdateRunnerManagerRequest, WorkspaceBindingResponse},
};

use super::helpers::{IsolatedDb, create_app_with_token, test_state};
use super::http::{http_post, spawn_http};

/// The fixed internal marker a resource binding carries as its external user.
pub const RESOURCE_MARKER: &str = "__resource__";

pub fn binding_path(resource_kind: &str, resource_id: &str) -> String {
    format!("/api/internal/resource-workspaces/{resource_kind}/{resource_id}/binding")
}

/// One isolated database, one application bearer and one temporary workspace
/// root, served by the real internal router.
pub struct BindingFixture {
    pub database: IsolatedDb,
    pub base: String,
    pub application_id: i64,
    pub token: String,
    handle: tokio::task::JoinHandle<()>,
    _temp: tempfile::TempDir,
}

impl BindingFixture {
    pub async fn setup() -> Self {
        let database = IsolatedDb::setup().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let (application_id, token) = create_app_with_token(&database.pool, "binding-owner").await;
        let mut state = test_state(database.pool.clone());
        let mut config = (*state.config).clone();
        config.workspace_root = temp.path().to_path_buf();
        state.config = Arc::new(config);
        let (base, handle) = spawn_http(state).await;
        Self {
            database,
            base,
            application_id,
            token,
            handle,
            _temp: temp,
        }
    }

    /// A second application on the same isolated database.
    pub async fn other_application(&self, name: &str) -> (i64, String) {
        create_app_with_token(&self.database.pool, name).await
    }

    /// Resolve one resource binding over real HTTP.
    pub async fn resolve(&self, token: Option<&str>, kind: &str, id: &str) -> (u16, Value) {
        http_post(
            &self.base,
            &binding_path(kind, id),
            token,
            serde_json::json!({}),
        )
        .await
    }

    /// The stored row behind one binding id.
    pub async fn row(&self, binding_id: i64) -> WorkspaceBindingResponse {
        queries::find_workspace_binding_by_id(&self.database.pool, binding_id)
            .await
            .expect("read binding")
            .expect("binding exists")
    }

    /// The resource binding row for one key, if any.
    pub async fn resource_row(&self, kind: &str, id: &str) -> Option<WorkspaceBindingResponse> {
        queries::find_workspace_binding_by_resource(
            &self.database.pool,
            self.application_id,
            kind,
            id,
        )
        .await
        .expect("read resource binding")
    }

    /// One enabled, online runner manager: a task intent needs one to bind to,
    /// and admission itself never starts a container.
    pub async fn online_manager(&self, name: &str) -> i64 {
        let (record, _token) = queries::create_runner_manager(
            &self.database.pool,
            &CreateRunnerManagerRequest {
                name: name.to_string(),
                endpoint: format!("http://{name}.invalid"),
                access_token: None,
                image: "test-image".to_string(),
                network_enabled: false,
                max_output_bytes: 65_536,
                max_timeout_ms: 600_000,
                max_sessions: 2,
                pids_limit: 64,
                memory_limit: "256m".to_string(),
                cpu_limit: "1".to_string(),
                host_workspace_root: None,
            },
        )
        .await
        .expect("create runner manager");
        let manager_id = record.runner_manager_id;
        queries::update_runner_manager(
            &self.database.pool,
            manager_id,
            &UpdateRunnerManagerRequest {
                endpoint: format!("http://{name}.invalid"),
                enabled: true,
                image: "test-image".to_string(),
                network_enabled: false,
                max_output_bytes: 65_536,
                max_timeout_ms: 600_000,
                max_sessions: 2,
                pids_limit: 64,
                memory_limit: "256m".to_string(),
                cpu_limit: "1".to_string(),
                host_workspace_root: None,
            },
        )
        .await
        .expect("enable runner manager");
        queries::touch_runner_manager(&self.database.pool, manager_id)
            .await
            .expect("mark runner manager online");
        manager_id
    }

    /// Admit one native task intent through the real admission path.
    pub async fn admit(
        &self,
        binding_id: i64,
        subject: &str,
        task_id: Uuid,
    ) -> Result<NativeTask, NativeTaskError> {
        let environment_id = self.ensure_environment(subject).await;
        admit_native_task(
            &self.database.pool,
            self.application_id,
            environment_id,
            NativeTaskRequest {
                task_id,
                workspace_binding_id: binding_id,
                subject_external_user_id: subject.to_string(),
                expected_generation: 0,
                operation_token: format!("operation-{task_id}"),
            },
        )
        .await
    }

    /// One fresh environment at generation zero, without any lifecycle job.
    async fn ensure_environment(&self, subject: &str) -> Uuid {
        match ensure_execution_environment(
            &self.database.pool,
            self.application_id,
            subject,
            "opencode",
        )
        .await
        .expect("ensure environment")
        {
            EnsureOutcome::Created(environment) | EnsureOutcome::Existing(environment) => {
                environment.environment_id
            }
            EnsureOutcome::RuntimeConflict { .. } => {
                panic!("a fresh subject must not conflict")
            }
        }
    }

    /// The environment one admitted task belongs to.
    pub fn pool(&self) -> &sqlx::PgPool {
        &self.database.pool
    }

    pub async fn teardown(self) {
        self.handle.abort();
        self.database.teardown().await;
    }
}

/// The instant a JSON timestamp cell carries, for row/body comparisons.
pub fn instant(value: &Value) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|at| at.with_timezone(&chrono::Utc))
}

/// Assert that one binding carries no legacy write lease at all.
pub fn assert_lease_free(binding: &WorkspaceBindingResponse) {
    assert!(
        binding.write_lease_owner.is_none(),
        "binding resolution must not acquire a lease owner"
    );
    assert!(
        binding.write_lease_acquired_at.is_none(),
        "binding resolution must not stamp a lease acquisition"
    );
    assert!(
        binding.write_lease_expires_at.is_none(),
        "binding resolution must not set a lease expiry"
    );
}
