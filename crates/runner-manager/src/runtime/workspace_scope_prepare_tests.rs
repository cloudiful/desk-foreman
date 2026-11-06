use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicUsize, Ordering},
    },
    time::Duration,
};

use desk_foreman::runner::{RunnerFuture, RunnerService};
use runner_protocol::{
    CancelSessionRequest, CommandOutput, EnsureExecutionEnvironmentRequest, ExecRequest,
    RunnerCommandRequest, RunnerOwner, RunnerShellRequest,
};
use tokio::sync::RwLock;

use super::LocalRunnerService;
use crate::config::SharedRunnerManagerConfig;
use crate::runtime::{
    DirectRunnerBackend, ProcessSpawnTarget, RunnerBackend, docker_tests::test_config,
};

#[derive(Default)]
struct RecordingBackend {
    activity: AtomicU32,
    quiesce_calls: AtomicUsize,
    supports: std::sync::Mutex<bool>,
    direct: Option<Arc<DirectRunnerBackend>>,
}

impl RecordingBackend {
    fn new() -> Self {
        Self {
            supports: std::sync::Mutex::new(true),
            ..Self::default()
        }
    }

    fn with_direct_shell() -> Self {
        Self {
            direct: Some(DirectRunnerBackend::new()),
            ..Self::new()
        }
    }

    fn set_activity(&self, value: u32) {
        self.activity.store(value, Ordering::SeqCst);
    }

    fn set_supports(&self, value: bool) {
        *self.supports.lock().unwrap() = value;
    }
}

impl RunnerBackend for RecordingBackend {
    fn reconcile<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn prepare_shell_spawn<'a>(
        &'a self,
        request: RunnerShellRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ProcessSpawnTarget>> {
        match &self.direct {
            Some(direct) => direct.prepare_shell_spawn(request),
            None => Box::pin(async { anyhow::bail!("unused") }),
        }
    }

    fn run_command<'a>(
        &'a self,
        _request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
        Box::pin(async {
            Ok(CommandOutput {
                wall_time_seconds: 0.0,
                output: String::new(),
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(0),
                truncated: false,
                timed_out: false,
                stdout_bytes: 0,
                stderr_bytes: 0,
            })
        })
    }

    fn cleanup_runner_owner<'a>(
        &'a self,
        _owner: RunnerOwner,
    ) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async { anyhow::bail!("unused") })
    }

    fn supports_workspace_binding_owner_quiescence(&self) -> bool {
        *self.supports.lock().unwrap()
    }

    fn quiesce_workspace_binding_owners<'a>(
        &'a self,
        _workspace_binding_id: i64,
        _authorized_local_workspace_root: PathBuf,
        _expected_own: Option<crate::runtime::backend::ExpectedOwnContainer>,
    ) -> RunnerFuture<'a, anyhow::Result<()>> {
        self.quiesce_calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }

    fn workspace_activity_count<'a>(
        &'a self,
        _workspace_binding_id: i64,
    ) -> RunnerFuture<'a, anyhow::Result<u32>> {
        let value = self.activity.load(Ordering::SeqCst);
        Box::pin(async move { Ok(value) })
    }
}

fn local_service(backend: Arc<RecordingBackend>) -> Arc<LocalRunnerService> {
    let config: SharedRunnerManagerConfig = Arc::new(RwLock::new(test_config()));
    LocalRunnerService::new(backend, config)
}

fn command(binding_id: i64) -> RunnerCommandRequest {
    RunnerCommandRequest {
        owner: RunnerOwner::WorkspaceBinding {
            workspace_binding_id: binding_id,
        },
        workspace_root: PathBuf::from("/tmp"),
        working_dir: PathBuf::from("/tmp"),
        program: "true".to_string(),
        args: Vec::new(),
        timeout_ms: None,
        max_output_bytes: None,
        network_enabled: false,
    }
}

#[tokio::test]
async fn prepare_workspace_scope_returns_a_held_guard_and_second_prepare_is_busy() {
    let backend = Arc::new(RecordingBackend::new());
    let service = local_service(backend.clone());
    let guard = service
        .acquire_workspace_scope(5)
        .await
        .expect("clear binding must prepare");
    assert_eq!(
        backend.quiesce_calls.load(Ordering::SeqCst),
        0,
        "admission must not enter ordinary-container quiescence"
    );

    let error = service
        .acquire_workspace_scope(5)
        .await
        .expect_err("a held preparation must refuse a second one");
    assert!(error.to_string().contains("busy"));
    assert_eq!(
        backend.quiesce_calls.load(Ordering::SeqCst),
        0,
        "a busy fence must not reach backend quiescence"
    );

    // The held guard blocks same-binding ordinary RPC but allows a sibling.
    let same = service.clone();
    let blocked = tokio::time::timeout(Duration::from_millis(100), async move {
        same.run_command(command(5)).await
    })
    .await;
    assert!(blocked.is_err(), "same-binding RPC must block");
    let sibling = service.clone();
    let allowed = tokio::time::timeout(Duration::from_secs(1), async move {
        sibling.run_command(command(6)).await
    })
    .await
    .expect("sibling RPC must not block")
    .expect("sibling RPC must succeed");
    assert_eq!(allowed.exit_code, Some(0));

    drop(guard);
    service
        .acquire_workspace_scope(5)
        .await
        .expect("dropping the guard must release the binding");
}

#[tokio::test]
async fn prepare_workspace_scope_refuses_active_operations_without_quiescing() {
    let backend = Arc::new(RecordingBackend::new());
    backend.set_activity(1);
    let service = local_service(backend.clone());
    assert!(service.acquire_workspace_scope(5).await.is_err());
    assert_eq!(backend.quiesce_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn prepare_workspace_scope_refuses_a_held_fence_or_unsupported_backend() {
    let backend = Arc::new(RecordingBackend::new());
    let service = local_service(backend.clone());
    let fence = service
        .binding_fences
        .for_owner(&RunnerOwner::WorkspaceBinding {
            workspace_binding_id: 5,
        });
    let held_read = fence.clone().read_owned().await;
    assert!(service.acquire_workspace_scope(5).await.is_err());
    drop(held_read);
    assert_eq!(backend.quiesce_calls.load(Ordering::SeqCst), 0);

    let held_write = fence.write_owned().await;
    assert!(service.acquire_workspace_scope(5).await.is_err());
    drop(held_write);
    assert_eq!(backend.quiesce_calls.load(Ordering::SeqCst), 0);

    backend.set_supports(false);
    assert!(service.acquire_workspace_scope(5).await.is_err());
    assert_eq!(backend.quiesce_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn acquire_workspace_scope_drops_the_guard_after_an_admission_error() {
    let backend = Arc::new(RecordingBackend::new());
    backend.set_activity(1);
    let service = local_service(backend.clone());
    assert!(service.acquire_workspace_scope(5).await.is_err());

    // Same service: a new instance would have an independent fence map and
    // would not prove the original admission guard was released.
    backend.set_activity(0);
    let guard = service
        .acquire_workspace_scope(5)
        .await
        .expect("the error path must release the original binding fence");
    drop(guard);
    assert_eq!(backend.quiesce_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn prepare_workspace_scope_preserves_a_live_workspace_binding_shell() {
    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(RecordingBackend::with_direct_shell());
    let service = local_service(backend.clone());
    let owner = RunnerOwner::WorkspaceBinding {
        workspace_binding_id: 5,
    };
    let started = service
        .exec_shell(ExecRequest {
            owner: owner.clone(),
            session_key: None,
            workspace_root: temp.path().to_path_buf(),
            cmd: "sleep 30".to_string(),
            workdir: None,
            shell: "sh".to_string(),
            login: false,
            tty: false,
            timeout_ms: None,
            yield_time_ms: Some(100),
            max_output_tokens: None,
            max_output_bytes: None,
            network_enabled: false,
        })
        .await
        .expect("a long-lived shell must start");
    let session_id = started
        .session_id
        .expect("a running shell yields a session");

    let result = service.acquire_workspace_scope(5).await;
    let refused = result.is_err();
    let returned_guard = result.ok();
    let preserved = service
        .shell
        .snapshot_sessions()
        .await
        .expect("snapshot sessions")
        .iter()
        .any(|session| session.session_id == session_id);
    let quiesce_calls = backend.quiesce_calls.load(Ordering::SeqCst);

    // A faulty successful guard must be released before cleanup, otherwise
    // cancellation would deadlock on the still-held binding fence.
    if let Some(guard) = returned_guard {
        drop(guard);
    }
    service
        .cancel_session(CancelSessionRequest {
            owner,
            session_key: None,
            session_id,
        })
        .await
        .expect("cancel the fixture shell");
    let after = service
        .shell
        .snapshot_sessions()
        .await
        .expect("snapshot after cancel");

    assert!(refused, "a live session must refuse preparation");
    assert!(
        preserved,
        "the primitive must not terminate the live session"
    );
    assert_eq!(
        quiesce_calls, 0,
        "the backend quiescence primitive must not be entered"
    );
    assert!(!after.iter().any(|session| session.session_id == session_id));
}

#[tokio::test]
async fn some_scope_ensure_holds_the_binding_fence_and_reaches_the_backend() {
    let backend = Arc::new(RecordingBackend::new());
    let service = local_service(backend.clone());
    let request = EnsureExecutionEnvironmentRequest {
        owner: RunnerOwner::ExecutionEnvironment {
            environment_id: "123e4567-e89b-12d3-a456-426614174000".to_string(),
            generation: 1,
        },
        workspace_scope: Some(
            runner_protocol::execution_environment::ExecutionEnvironmentWorkspaceScope {
                workspace_binding_id: 5,
                workspace_relative_path: "scope".to_string(),
            },
        ),
    };
    assert!(service.ensure_execution_environment(request).await.is_err());
    assert_eq!(backend.quiesce_calls.load(Ordering::SeqCst), 0);
}
