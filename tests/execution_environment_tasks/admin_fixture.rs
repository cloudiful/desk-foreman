//! Admin-transition fixture for the native-task suite (issue 753 P1).
//!
//! Split so the admin-transition cases and the janitor cases share one
//! definition of the fixture instead of duplicating it. It is local and
//! in process: the real router, the real handlers and the isolated database,
//! with the admin session minted over the real login route. No live admin
//! server, operator console or remote call is involved.

use desk_foreman::{AppState, lifecycle::cleanup_archived_workspaces};
use uuid::Uuid;

use super::support::{
    HttpOutcome, TaskFixture, admin_session, call, create_code_project_binding,
    test_state_with_admin,
};

const ADMIN_LOGIN: &str = "task-admin";
const ADMIN_PASSWORD: &str = "native-task-admin-password";

/// One admin-driven fixture: a native task holder plus a second, independent
/// workspace the admin transitions operate on.
pub struct AdminFixture {
    pub fixture: TaskFixture,
    /// Router bound to the admin-bootstrapped state.
    admin_base: String,
    pub admin_state: AppState,
    pub other_binding_id: i64,
    pub other_root: String,
    cookie: String,
    _admin_temp: tempfile::TempDir,
}

impl AdminFixture {
    /// One application, one environment, the admin state and a second
    /// workspace that carries no task of its own.
    pub async fn setup() -> Self {
        let fixture = TaskFixture::setup().await;
        let admin_temp = tempfile::tempdir().expect("admin tempdir");
        let workspace_root = admin_temp.path().join("workspaces");
        std::fs::create_dir_all(&workspace_root).expect("workspace root");
        let admin_state = test_state_with_admin(
            fixture.database.pool.clone(),
            &workspace_root.to_string_lossy(),
            ADMIN_LOGIN,
            ADMIN_PASSWORD,
        )
        .await;
        let (admin_base, _handle) = super::http::spawn_http(admin_state.clone()).await;
        let cookie = admin_session(&admin_base, ADMIN_LOGIN, ADMIN_PASSWORD).await;

        let other_root = workspace_root.join("other-workspace");
        std::fs::create_dir_all(&other_root).expect("other workspace");
        std::fs::write(other_root.join("keep.txt"), b"keep me").expect("seed file");
        let other_binding_id = create_code_project_binding(
            &fixture.database.pool,
            fixture.application_id,
            &other_root.to_string_lossy(),
            "code_project:other",
        )
        .await;

        Self {
            fixture,
            admin_base,
            admin_state,
            other_binding_id,
            other_root: other_root.to_string_lossy().to_string(),
            cookie,
            _admin_temp: admin_temp,
        }
    }

    pub async fn admin_post(&self, path: &str) -> HttpOutcome {
        call(
            &self.admin_base,
            "POST",
            path,
            None,
            &[("Cookie".to_string(), self.cookie.clone())],
            b"",
        )
        .await
    }

    /// Archive one binding directly, far enough in the past for the janitor.
    pub async fn archive_for_janitor(&self, binding_id: i64) {
        sqlx::query(
            "UPDATE workspace_bindings SET lifecycle_state = 'archived', is_active = FALSE, \
             archived_at = NOW() - INTERVAL '400 days' WHERE workspace_binding_id = $1",
        )
        .bind(binding_id)
        .execute(&self.fixture.database.pool)
        .await
        .expect("archive binding for the janitor");
    }

    pub async fn binding_state(&self, binding_id: i64) -> (String, bool) {
        sqlx::query_as(
            "SELECT lifecycle_state, is_active FROM workspace_bindings WHERE workspace_binding_id = $1",
        )
        .bind(binding_id)
        .fetch_one(&self.fixture.database.pool)
        .await
        .expect("binding row")
    }

    pub async fn binding_exists(&self, binding_id: i64) -> bool {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT workspace_binding_id FROM workspace_bindings WHERE workspace_binding_id = $1",
        )
        .bind(binding_id)
        .fetch_optional(&self.fixture.database.pool)
        .await
        .expect("binding probe");
        row.is_some()
    }

    /// Admit a task that owns `binding_id`'s workspace.
    pub async fn admit_on(&self, binding_id: i64, operation_token: &str) {
        let outcome = self
            .fixture
            .admit(
                &self.fixture.application_token,
                Uuid::new_v4(),
                binding_id,
                "task-user",
                4,
                operation_token,
            )
            .await;
        assert_eq!(outcome.status, 200, "admit: {}", outcome.body_text());
    }

    /// Run one real janitor sweep against the admin-bootstrapped state.
    pub async fn run_janitor(&self) {
        cleanup_archived_workspaces(&self.admin_state)
            .await
            .expect("janitor sweep");
    }
}
