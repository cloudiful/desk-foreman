//! Rig and fixtures for the mandatory native-task preparation gate
//! (issue 753 P2b).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use desk_foreman::db::execution_environments::tasks::seeds::plan_task_seed;
use desk_foreman::db::execution_environments::tasks::{
    NativeTaskRequest, PrepareRequest, admit_native_task, prepare_native_task,
};
use runner_protocol::{PrepareNativeTaskRequest, RunnerJob, SourceSeedSnapshotFile};
use sqlx::PgPool;
use uuid::Uuid;

use crate::config::{RunnerBackendKind, RunnerManagerConfig};
use crate::runtime::execution_environment::service::host_identity::UidSource;
use crate::runtime::{DockerRunnerBackend, NativeTaskStore, RunnerLifecycleReporter};
use crate::upstream_service_relay::full_gateway_db::db::IsolatedDb;
use crate::upstream_service_relay::full_gateway_support::support::{
    create_app, create_manager, spawn_df, test_state,
};

/// How long a gate request may take.
pub(super) const GATE_TIMEOUT: Duration = Duration::from_secs(20);

/// One preparation rig with a real router and a real trusted root.
pub(super) struct Rig {
    pub(super) db: IsolatedDb,
    pub(super) base: String,
    _handle: tokio::task::JoinHandle<()>,
    pub(super) manager_id: i64,
    pub(super) manager_token: String,
    pub(super) app_id: i64,
    pub(super) env_id: Uuid,
    pub(super) task_id: Uuid,
    pub(super) root: PathBuf,
    _temp: tempfile::TempDir,
    pub(super) client: reqwest::Client,
}

/// The current process's effective uid.
pub(super) fn current_uid() -> u32 {
    UidSource::Proc.effective_uid().expect("effective uid")
}

/// An owner-only trusted root under a fresh temporary directory.
pub(super) fn root(temp: &tempfile::TempDir) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = temp.path().join("native-root");
    std::fs::create_dir(&path).expect("root dir");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).expect("mode");
    path
}

/// A manager config for the backend, with the given root and identity.
pub(super) fn manager_config(
    root: Option<PathBuf>,
    manager_id: i64,
) -> Arc<tokio::sync::RwLock<RunnerManagerConfig>> {
    Arc::new(tokio::sync::RwLock::new(RunnerManagerConfig {
        control_plane_url: None,
        manager_id: manager_id.to_string(),
        bind_addr: "127.0.0.1:0".to_string(),
        auth_token: "test-token".to_string(),
        backend: RunnerBackendKind::Docker,
        workspace_root: PathBuf::from("/tmp"),
        host_workspace_root: PathBuf::from("/tmp"),
        image: "test-image".to_string(),
        workdir: "/workspace".to_string(),
        network_enabled: false,
        max_output_bytes: 65_536,
        max_timeout_ms: 600_000,
        max_sessions: 2,
        pids_limit: 64,
        memory_limit: "256m".to_string(),
        cpu_limit: "1".to_string(),
        idle_ttl: Duration::from_secs(60),
        docker_cli: "docker".to_string(),
        docker_host: None,
        runtime_class: None,
        native_task_root: root,
    }))
}

/// Create a manager and mark it usable, so the preparation fences pass.
pub(super) async fn online_manager(pool: &PgPool, name: &str) -> (i64, String) {
    let (manager_id, token) = create_manager(pool, name).await;
    sqlx::query(
        "UPDATE runner_managers \
         SET enabled = TRUE, status = 'online', last_seen_at = NOW() \
         WHERE runner_manager_id = $1",
    )
    .bind(manager_id)
    .execute(pool)
    .await
    .expect("mark manager online");
    (manager_id, token)
}

/// The canonical plan for a `(path, content)` list.
pub(super) fn plan_of(files: &[(&str, &str)]) -> runner_protocol::SeedBundle {
    let entries: Vec<SourceSeedSnapshotFile> = files
        .iter()
        .map(|(path, content)| SourceSeedSnapshotFile::new(*path, *content))
        .collect();
    plan_task_seed(&entries).expect("plan")
}

/// Ensure the environment and pin it into the exact idle shape a task may bind.
async fn prepare_environment(pool: &PgPool, app_id: i64, manager_id: i64) -> Uuid {
    let outcome = desk_foreman::db::queries::ensure_execution_environment(
        pool,
        app_id,
        "prep-user",
        "opencode",
    )
    .await
    .expect("ensure environment");
    let env_id = match outcome {
        desk_foreman::db::execution_environments::EnsureOutcome::Created(env)
        | desk_foreman::db::execution_environments::EnsureOutcome::Existing(env) => {
            env.environment_id
        }
        _ => panic!("fresh environment must not conflict"),
    };
    sqlx::query(
        "UPDATE execution_environments \
         SET generation = 4, container_generation = NULL, assigned_manager_id = $2, \
             state = 'absent', stop_proof_job_id = NULL, stop_proof_generation = NULL, \
             active_native_task_id = NULL \
         WHERE environment_id = $1",
    )
    .bind(env_id)
    .bind(manager_id)
    .execute(pool)
    .await
    .expect("write environment fence");
    env_id
}

/// Create one live code-project resource workspace binding.
async fn code_project_binding(pool: &PgPool, app_id: i64) -> i64 {
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO workspace_bindings ( \
             application_id, external_user_id, workspace_key, external_user_hash, \
             workspace_root, is_active, lifecycle_state, resource_kind, resource_id \
         ) VALUES ($1, 'prep-user', 'code_project:alpha', 'hash', '/tmp/ws', TRUE, 'active', 'code_project', 'alpha') \
         RETURNING workspace_binding_id",
    )
    .bind(app_id)
    .fetch_one(pool)
    .await
    .expect("binding");
    id
}

impl Rig {
    /// Build an admitted, seeded task whose preparation is scheduled.
    pub(super) async fn start(files: &[(&str, &str)]) -> Self {
        let db = IsolatedDb::setup().await;
        let (app_id, _app_token) = create_app(&db.pool, "prep-owner").await;
        let (manager_id, manager_token) = online_manager(&db.pool, "prep-manager").await;
        let env_id = prepare_environment(&db.pool, app_id, manager_id).await;
        let binding_id = code_project_binding(&db.pool, app_id).await;
        let temp = tempfile::tempdir().expect("tempdir");
        let state = test_state(db.pool.clone());
        let (base, handle) = spawn_df(state).await;

        let task_id = Uuid::new_v4();
        admit_native_task(
            &db.pool,
            app_id,
            env_id,
            NativeTaskRequest {
                task_id,
                workspace_binding_id: binding_id,
                subject_external_user_id: "prep-user".to_string(),
                expected_generation: 4,
                operation_token: "op-admit".to_string(),
            },
        )
        .await
        .expect("admit task");

        let plan = plan_of(files);
        seal_seed(&db.pool, app_id, env_id, task_id, &plan).await;
        prepare_native_task(
            &db.pool,
            app_id,
            env_id,
            task_id,
            PrepareRequest {
                expected_generation: 4,
                operation_token: "op-prepare".to_string(),
            },
        )
        .await
        .expect("schedule preparation");

        Self {
            root: root(&temp),
            _temp: temp,
            client: reqwest::Client::new(),
            db,
            base,
            _handle: handle,
            manager_id,
            manager_token,
            app_id,
            env_id,
            task_id,
        }
    }

    /// A backend with a real filesystem store over `root`.
    pub(super) fn backend(&self, root: Option<PathBuf>) -> Arc<DockerRunnerBackend> {
        let store = NativeTaskStore::with_uid_source(root.clone(), UidSource::Fixed(current_uid()));
        let config = manager_config(root, self.manager_id);
        DockerRunnerBackend::new(
            config,
            self.manager_id.to_string(),
            RunnerLifecycleReporter::noop(),
            store,
        )
    }

    /// Poll the durable job as the manager.
    pub(super) async fn poll(&self) -> RunnerJob {
        let response = self
            .client
            .get(format!(
                "{}/api/internal/runner-manager/jobs/next",
                self.base
            ))
            .bearer_auth(&self.manager_token)
            .timeout(GATE_TIMEOUT)
            .send()
            .await
            .expect("poll");
        assert_eq!(response.status(), 200);
        let job: Option<RunnerJob> = response.json().await.expect("job json");
        job.expect("a preparation job is claimable")
    }

    /// The task's current state.
    pub(super) async fn task_state(&self) -> String {
        let (state,): (String,) =
            sqlx::query_as("SELECT state FROM execution_environment_tasks WHERE task_id = $1")
                .bind(self.task_id)
                .fetch_one(&self.db.pool)
                .await
                .expect("task state");
        state
    }

    /// The environment's candidate generation and state.
    pub(super) async fn environment_fence(&self) -> (Option<i64>, String) {
        sqlx::query_as(
            "SELECT container_generation, state FROM execution_environments WHERE environment_id = $1",
        )
        .bind(self.env_id)
        .fetch_one(&self.db.pool)
        .await
        .expect("environment fence")
    }

    /// The trusted tree directory of this task.
    pub(super) fn tree_dir(&self) -> PathBuf {
        self.root
            .join(self.env_id.to_string())
            .join(self.task_id.to_string())
            .join("tree")
    }
}

/// Seal the plan, panicking if the environment refuses it.
async fn seal_seed(
    pool: &PgPool,
    app_id: i64,
    env_id: Uuid,
    task_id: Uuid,
    plan: &runner_protocol::SeedBundle,
) {
    use desk_foreman::db::execution_environments::tasks::seeds::seal_native_task_seed;
    seal_native_task_seed(pool, app_id, env_id, task_id, 4, plan)
        .await
        .expect("seal seed");
}

/// Parse a preparation request out of a polled job payload.
pub(super) fn prepare_request(job: &RunnerJob) -> PrepareNativeTaskRequest {
    serde_json::from_value(job.payload.clone()).expect("prepare request")
}
