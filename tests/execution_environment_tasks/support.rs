use uuid::Uuid;

use super::helpers::{IsolatedDb, create_app_with_token};
use super::http::spawn_http;

// Re-exported so the case files keep one import surface: `super::support::*`
// for everything, with the owning modules free to stay small.
pub use super::http_client::{HttpOutcome, call};

pub use super::admin_state::{admin_session, test_state_with_admin};
pub use super::db_fixtures::{
    create_code_project_binding, create_environment, create_manager, set_cold_fence,
    set_idle_fence, set_legacy_lease_owner,
};
pub use super::task_seed::{active_task_of, confirmed_stop, seed_executed_task, stored_state};

pub const RESOURCE_MARKER: &str = "resource-shared";

/// Build the `(path, content)` list a seed request carries.
pub fn seed_files(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(path, content)| (path.to_string(), content.to_string()))
        .collect()
}

/// One admitted environment, its application identity and its workspace.
pub struct TaskFixture {
    pub database: IsolatedDb,
    pub base: String,
    pub application_id: i64,
    pub application_token: String,
    pub other_application_token: String,
    pub manager_id: i64,
    /// The manager's own bearer token, for the manager result route.
    pub manager_token: String,
    pub environment_id: Uuid,
    pub binding_id: i64,
    pub relay: std::sync::Arc<desk_foreman::runner::service_relay::RelayRegistry>,
    _temp: tempfile::TempDir,
}

impl TaskFixture {
    /// One application, one environment bound to a live manager, and one
    /// authorized code-project workspace. The environment has no candidate, so
    /// a task intent is admissible.
    pub async fn setup() -> Self {
        Self::build(false).await
    }

    /// The same fixture on the genuinely cold path (issue 824 P1): the
    /// environment keeps `assigned_manager_id = NULL`, exactly as a public
    /// ensure leaves it. No helper writes a manager onto the environment.
    pub async fn setup_cold() -> Self {
        Self::build(true).await
    }

    async fn build(cold: bool) -> Self {
        let database = IsolatedDb::setup().await;
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace_root = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace_root).expect("workspace root");
        let root = workspace_root.to_string_lossy().to_string();
        let (application_id, application_token) =
            create_app_with_token(&database.pool, "task-owner").await;
        let (_other_id, other_application_token) =
            create_app_with_token(&database.pool, "task-other").await;
        let (manager_id, manager_token) = create_manager(&database.pool, "task-manager").await;
        let environment_id = create_environment(&database.pool, application_id, "task-user").await;
        if cold {
            set_cold_fence(&database.pool, environment_id, 4).await
        } else {
            set_idle_fence(&database.pool, environment_id, manager_id, 4).await
        }
        let binding_id = create_code_project_binding(
            &database.pool,
            application_id,
            &root,
            "code_project:alpha",
        )
        .await;
        let mut state = super::helpers::test_state(database.pool.clone());
        let mut config = (*state.config).clone();
        config.workspace_root = temp.path().to_path_buf();
        state.config = std::sync::Arc::new(config);
        let relay = state.runner_broker.service_relay().clone();
        let (base, _handle) = spawn_http(state).await;
        Self {
            database,
            base,
            application_id,
            application_token,
            other_application_token,
            manager_id,
            manager_token,
            environment_id,
            binding_id,
            relay,
            _temp: temp,
        }
    }

    pub fn admit_path(&self) -> String {
        format!(
            "/api/internal/execution-environments/{}/tasks/admit",
            self.environment_id
        )
    }

    pub fn task_path(&self, task_id: Uuid) -> String {
        format!(
            "/api/internal/execution-environments/{}/tasks/{task_id}",
            self.environment_id
        )
    }

    pub fn cancel_path(&self, task_id: Uuid) -> String {
        format!("{}/cancel", self.task_path(task_id))
    }

    pub fn release_path(&self, task_id: Uuid) -> String {
        format!("{}/release", self.task_path(task_id))
    }

    /// One admit call with the fence header every task request needs.
    pub async fn admit(
        &self,
        token: &str,
        task_id: Uuid,
        binding_id: i64,
        subject: &str,
        generation: i64,
        operation_token: &str,
    ) -> HttpOutcome {
        call(
            &self.base,
            "POST",
            &self.admit_path(),
            Some(token),
            &[fence_header(generation)],
            serde_json::to_vec(&serde_json::json!({
                "task_id": task_id,
                "workspace_binding_id": binding_id,
                "subject_external_user_id": subject,
                "expected_generation": generation,
                "operation_token": operation_token,
            }))
            .expect("admit body")
            .as_slice(),
        )
        .await
    }

    /// Admit with this fixture's own identity, subject and binding.
    pub async fn admit_default(&self, task_id: Uuid, operation_token: &str) -> HttpOutcome {
        self.admit(
            &self.application_token,
            task_id,
            self.binding_id,
            "task-user",
            4,
            operation_token,
        )
        .await
    }

    pub async fn json_post(
        &self,
        token: &str,
        path: &str,
        generation: i64,
        body: serde_json::Value,
    ) -> HttpOutcome {
        call(
            &self.base,
            "POST",
            path,
            Some(token),
            &[fence_header(generation)],
            serde_json::to_vec(&body).expect("body").as_slice(),
        )
        .await
    }

    /// The seed route for one task.
    pub fn seed_path(&self, task_id: Uuid) -> String {
        format!("{}/seed", self.task_path(task_id))
    }

    /// Seal a snapshot with `files` exactly as given (ordered JSON array).
    pub async fn seed(
        &self,
        token: &str,
        task_id: Uuid,
        generation: i64,
        files: &[(String, String)],
    ) -> HttpOutcome {
        let files: Vec<serde_json::Value> = files
            .iter()
            .map(|(path, content)| serde_json::json!({ "path": path, "content": content }))
            .collect();
        self.seed_raw(
            token,
            task_id,
            serde_json::to_vec(&serde_json::json!({
                "expected_generation": generation,
                "files": files,
            }))
            .expect("seed body"),
        )
        .await
    }

    /// Seal a snapshot from an exactly-supplied request body, for size probes.
    pub async fn seed_raw(&self, token: &str, task_id: Uuid, body: Vec<u8>) -> HttpOutcome {
        call(
            &self.base,
            "POST",
            &self.seed_path(task_id),
            Some(token),
            &[fence_header(0)],
            &body,
        )
        .await
    }

    /// Report one durable job result as the manager would.
    pub async fn manager_result(
        &self,
        job_id: Uuid,
        ok: bool,
        result: serde_json::Value,
    ) -> HttpOutcome {
        call(
            &self.base,
            "POST",
            "/api/internal/runner-manager/jobs/result",
            Some(&self.manager_token),
            &[],
            serde_json::to_vec(&serde_json::json!({
                "job_id": format!("envjob:{job_id}"),
                "ok": ok,
                "result": result,
                "error": if ok { serde_json::Value::Null } else { serde_json::json!("failed") },
            }))
            .expect("body")
            .as_slice(),
        )
        .await
    }

    /// The preparation route for one task.
    pub fn prepare_path(&self, task_id: Uuid) -> String {
        format!("{}/prepare", self.task_path(task_id))
    }

    /// Read one task's sealed snapshot over the real route.
    pub async fn get_seed(&self, token: &str, task_id: Uuid) -> HttpOutcome {
        self.get(token, &self.seed_path(task_id)).await
    }

    /// One release call with the given fences, against the fixture's own
    /// identity and this fixture's application token.
    pub async fn release(
        &self,
        task_id: Uuid,
        proof: Uuid,
        generation: i64,
        container_generation: i64,
    ) -> HttpOutcome {
        self.json_post(
            &self.application_token,
            &self.release_path(task_id),
            generation,
            serde_json::json!({
                "expected_generation": generation,
                "expected_container_generation": container_generation,
                "expected_stop_job_id": proof,
            }),
        )
        .await
    }

    pub async fn get(&self, token: &str, path: &str) -> HttpOutcome {
        call(&self.base, "GET", path, Some(token), &[], b"").await
    }

    pub async fn teardown(self) {
        self.database.teardown().await;
    }
}

/// The generation fence header every task request carries.
pub fn fence_header(generation: i64) -> (String, String) {
    (
        "X-Execution-Environment-Generation".to_string(),
        generation.to_string(),
    )
}
