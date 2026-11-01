use super::super::{DirectRunnerBackend, ProcessSpawnTarget, RunnerBackend};
use super::LocalRunnerService;
use desk_foreman::runner::{RunnerFuture, RunnerService};
use runner_protocol::{
    CommandOutput, ExecRequest, RunnerCommandRequest, RunnerOwner, RunnerShellRequest,
    StopWorkspacePrincipalAck, StopWorkspacePrincipalRequest, StopWorkspacePrincipalState,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::{Notify, RwLock};

struct PausedBackend {
    direct: Arc<DirectRunnerBackend>,
    entered: Notify,
    resume: Notify,
}
impl RunnerBackend for PausedBackend {
    fn reconcile<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<()>> {
        self.direct.reconcile()
    }
    fn prepare_shell_spawn<'a>(
        &'a self,
        request: RunnerShellRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ProcessSpawnTarget>> {
        self.direct.prepare_shell_spawn(request)
    }
    fn run_command<'a>(
        &'a self,
        request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
        self.direct.run_command(request)
    }
    fn cleanup_runner_owner<'a>(
        &'a self,
        owner: RunnerOwner,
    ) -> RunnerFuture<'a, anyhow::Result<()>> {
        self.direct.cleanup_runner_owner(owner)
    }
    fn supports_workspace_binding_principal_cleanup(&self) -> bool {
        true
    }
    fn cleanup_workspace_binding_principals<'a>(
        &'a self,
        _: runner_protocol::CleanupWorkspaceBindingPrincipalsRequest,
    ) -> RunnerFuture<'a, anyhow::Result<runner_protocol::CleanupWorkspaceBindingPrincipalsAck>>
    {
        Box::pin(async move {
            self.entered.notify_one();
            self.resume.notified().await;
            Ok(runner_protocol::CleanupWorkspaceBindingPrincipalsAck {
                state: StopWorkspacePrincipalState::Stopped,
                owners: Vec::new(),
                detail: None,
            })
        })
    }
    fn supports_workspace_principal_stop(&self) -> bool {
        true
    }
    fn stop_workspace_principal<'a>(
        &'a self,
        _: StopWorkspacePrincipalRequest,
    ) -> RunnerFuture<'a, anyhow::Result<StopWorkspacePrincipalAck>> {
        Box::pin(async move {
            self.entered.notify_one();
            self.resume.notified().await;
            Ok(StopWorkspacePrincipalAck {
                state: StopWorkspacePrincipalState::Stopped,
                detail: None,
            })
        })
    }
}

struct AdmissionAckBackend {
    direct: Arc<DirectRunnerBackend>,
    admitted: Notify,
    resume: Notify,
}
impl RunnerBackend for AdmissionAckBackend {
    fn reconcile<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<()>> {
        self.direct.reconcile()
    }
    fn prepare_shell_spawn<'a>(
        &'a self,
        request: RunnerShellRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ProcessSpawnTarget>> {
        Box::pin(async move {
            let target = self.direct.prepare_shell_spawn(request).await?;
            self.admitted.notify_one();
            self.resume.notified().await;
            Ok(target)
        })
    }
    fn run_command<'a>(
        &'a self,
        request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
        self.direct.run_command(request)
    }
    fn cleanup_runner_owner<'a>(
        &'a self,
        owner: RunnerOwner,
    ) -> RunnerFuture<'a, anyhow::Result<()>> {
        self.direct.cleanup_runner_owner(owner)
    }
    fn supports_workspace_principal_stop(&self) -> bool {
        true
    }
    fn stop_workspace_principal<'a>(
        &'a self,
        _: StopWorkspacePrincipalRequest,
    ) -> RunnerFuture<'a, anyhow::Result<StopWorkspacePrincipalAck>> {
        Box::pin(async {
            Ok(StopWorkspacePrincipalAck {
                state: StopWorkspacePrincipalState::Stopped,
                detail: None,
            })
        })
    }
}

#[tokio::test]
async fn acknowledged_admission_remains_fenced_until_spawn_is_registered() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = super::super::docker_tests::test_config();
    config.workspace_root = temp.path().into();
    config.host_workspace_root = temp.path().into();
    config.max_sessions = 4;
    let backend = Arc::new(AdmissionAckBackend {
        direct: DirectRunnerBackend::new(),
        admitted: Notify::new(),
        resume: Notify::new(),
    });
    let service = LocalRunnerService::new(backend.clone(), Arc::new(RwLock::new(config)));
    let spawn = tokio::spawn({
        let service = service.clone();
        let request = exec(owner(1), temp.path());
        async move { service.exec_shell(request).await }
    });
    tokio::time::timeout(Duration::from_secs(2), backend.admitted.notified())
        .await
        .unwrap();
    let held = service.shell.hold_session_map_for_test().await;
    backend.resume.notify_one();
    let mut stop = tokio::spawn({
        let service = service.clone();
        async move { service.stop_workspace_principal(request(owner(1))).await }
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut stop)
            .await
            .is_err()
    );
    let command = RunnerCommandRequest {
        owner: owner(2),
        workspace_root: temp.path().into(),
        working_dir: temp.path().into(),
        program: "true".into(),
        args: vec![],
        timeout_ms: None,
        max_output_bytes: None,
        network_enabled: false,
    };
    assert!(
        tokio::time::timeout(Duration::from_secs(2), service.run_command(command))
            .await
            .unwrap()
            .is_ok()
    );
    drop(held);
    spawn.await.unwrap().unwrap();
    assert_eq!(
        stop.await.unwrap().unwrap().state,
        StopWorkspacePrincipalState::Stopped
    );
}

fn owner(binding: i64) -> RunnerOwner {
    runner_protocol::workspace_principal::application_principal_owner(1, binding, "u", "w").unwrap()
}
fn request(owner: RunnerOwner) -> StopWorkspacePrincipalRequest {
    StopWorkspacePrincipalRequest {
        owner,
        principal_id: "p".into(),
    }
}
fn exec(owner: RunnerOwner, path: &std::path::Path) -> ExecRequest {
    ExecRequest {
        owner,
        session_key: Some("p".into()),
        workspace_root: path.into(),
        cmd: "true".into(),
        workdir: None,
        shell: "sh".into(),
        login: false,
        tty: false,
        timeout_ms: None,
        yield_time_ms: Some(20),
        max_output_tokens: None,
        max_output_bytes: None,
        network_enabled: false,
    }
}
fn setup(path: &std::path::Path) -> (Arc<LocalRunnerService>, Arc<PausedBackend>) {
    let backend = Arc::new(PausedBackend {
        direct: DirectRunnerBackend::new(),
        entered: Notify::new(),
        resume: Notify::new(),
    });
    let mut config = super::super::docker_tests::test_config();
    config.workspace_root = path.into();
    config.host_workspace_root = path.into();
    config.max_sessions = 4;
    (
        LocalRunnerService::new(backend.clone(), Arc::new(RwLock::new(config))),
        backend,
    )
}

#[tokio::test]
async fn long_canonical_key_stops_only_matching_owner_and_session() {
    let temp = tempfile::tempdir().unwrap();
    let (service, backend) = setup(temp.path());
    let external = "u".repeat(1024);
    let workspace = "code_project:resource";
    let target = runner_protocol::workspace_principal::application_principal_owner(
        1, 1, &external, workspace,
    )
    .unwrap();
    let other =
        runner_protocol::workspace_principal::application_principal_owner(1, 1, "other", workspace)
            .unwrap();
    let key = format!("application:1:{external}:{workspace}");
    assert!(key.len() > 1024);
    for (owner, session_key) in [
        (target.clone(), key.clone()),
        (other.clone(), "other-key".into()),
    ] {
        let mut req = exec(owner, temp.path());
        req.session_key = Some(session_key);
        req.cmd = "sleep 30".into();
        req.yield_time_ms = Some(1);
        service.exec_shell(req).await.unwrap().session_id.unwrap();
    }
    let wrong = StopWorkspacePrincipalRequest {
        owner: target.clone(),
        principal_id: format!("{key}x"),
    };
    assert_eq!(
        service.stop_workspace_principal(wrong).await.unwrap().state,
        StopWorkspacePrincipalState::Conflict
    );
    assert_eq!(service.shell.snapshot_sessions().await.unwrap().len(), 2);
    let stop = tokio::spawn({
        let service = service.clone();
        let target = target.clone();
        let key = key.clone();
        async move {
            service
                .stop_workspace_principal(StopWorkspacePrincipalRequest {
                    owner: target,
                    principal_id: key,
                })
                .await
                .unwrap()
        }
    });
    tokio::time::timeout(Duration::from_secs(2), backend.entered.notified())
        .await
        .unwrap();
    let remaining = service.shell.snapshot_sessions().await.unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].owner, other);
    let id = remaining[0].session_id;
    backend.resume.notify_one();
    assert_eq!(
        stop.await.unwrap().state,
        StopWorkspacePrincipalState::Stopped
    );
    service
        .cancel_session(runner_protocol::CancelSessionRequest {
            owner: other,
            session_key: Some("other-key".into()),
            session_id: id,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn binding_cleanup_drains_all_target_sessions_and_keeps_other_binding() {
    let temp = tempfile::tempdir().unwrap();
    let (service, backend) = setup(temp.path());
    let a = owner(1);
    let b =
        runner_protocol::workspace_principal::application_principal_owner(1, 1, "different", "w")
            .unwrap();
    let other = owner(2);
    for current in [a, b, other.clone()] {
        let mut req = exec(current, temp.path());
        req.cmd = "sleep 30".into();
        req.yield_time_ms = Some(1);
        service.exec_shell(req).await.unwrap().session_id.unwrap();
    }
    let cleaning = {
        let service = service.clone();
        tokio::spawn(async move {
            service
                .cleanup_workspace_binding_principals(
                    runner_protocol::CleanupWorkspaceBindingPrincipalsRequest {
                        workspace_binding_id: 1,
                    },
                )
                .await
                .unwrap()
        })
    };
    tokio::time::timeout(Duration::from_secs(2), backend.entered.notified())
        .await
        .unwrap();
    let sessions = service.shell.snapshot_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].owner, other);
    let id = sessions[0].session_id;
    backend.resume.notify_one();
    assert_eq!(
        cleaning.await.unwrap().state,
        StopWorkspacePrincipalState::Stopped
    );
    service
        .cancel_session(runner_protocol::CancelSessionRequest {
            owner: other,
            session_key: Some("p".into()),
            session_id: id,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn binding_cleanup_cancel_failure_never_calls_backend() {
    let temp = tempfile::tempdir().unwrap();
    let (service, backend) = setup(temp.path());
    let target = owner(1);
    let mut req = exec(target.clone(), temp.path());
    req.cmd = "sleep 30".into();
    req.yield_time_ms = Some(1);
    let id = service.exec_shell(req).await.unwrap().session_id.unwrap();
    service.shell.fail_next_cancel_for_test();
    let result = service
        .cleanup_workspace_binding_principals(
            runner_protocol::CleanupWorkspaceBindingPrincipalsRequest {
                workspace_binding_id: 1,
            },
        )
        .await
        .unwrap();
    assert_eq!(result.state, StopWorkspacePrincipalState::Failed);
    assert!(
        service
            .shell
            .snapshot_sessions()
            .await
            .unwrap()
            .iter()
            .any(|s| s.session_id == id)
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), backend.entered.notified())
            .await
            .is_err()
    );
    service
        .cancel_session(runner_protocol::CancelSessionRequest {
            owner: target,
            session_key: Some("p".into()),
            session_id: id,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn binding_cleanup_returns_busy_after_timeout_without_backend_side_effects() {
    let temp = tempfile::tempdir().unwrap();
    let (service, backend) = setup(temp.path());
    let binding = RunnerOwner::WorkspaceBinding {
        workspace_binding_id: 1,
    };
    let fence = service.binding_fences.for_owner(&binding);
    let _guard = fence.read().await;
    let before = std::time::Instant::now();
    let result = service
        .cleanup_workspace_binding_principals(
            runner_protocol::CleanupWorkspaceBindingPrincipalsRequest {
                workspace_binding_id: 1,
            },
        )
        .await
        .unwrap();
    assert_eq!(result.state, StopWorkspacePrincipalState::Busy);
    assert!(before.elapsed() >= Duration::from_secs(30));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), backend.entered.notified())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn binding_cleanup_direct_has_no_session_side_effects() {
    let temp = tempfile::tempdir().unwrap();
    let mut config = super::super::docker_tests::test_config();
    config.workspace_root = temp.path().into();
    config.host_workspace_root = temp.path().into();
    let service =
        LocalRunnerService::new(DirectRunnerBackend::new(), Arc::new(RwLock::new(config)));
    assert!(
        service
            .exec_shell(exec(owner(1), temp.path()))
            .await
            .is_err()
    );
    assert!(
        service
            .run_command(runner_protocol::RunnerCommandRequest {
                owner: owner(1),
                workspace_root: temp.path().into(),
                working_dir: temp.path().into(),
                program: "true".into(),
                args: vec![],
                timeout_ms: None,
                max_output_bytes: None,
                network_enabled: false,
            })
            .await
            .is_err()
    );
    let target = RunnerOwner::InternalUser { user_id: 1 };
    let mut running = exec(target.clone(), temp.path());
    running.cmd = "sleep 30".into();
    running.yield_time_ms = Some(1);
    let id = service
        .exec_shell(running)
        .await
        .unwrap()
        .session_id
        .unwrap();
    let result = service
        .cleanup_workspace_binding_principals(
            runner_protocol::CleanupWorkspaceBindingPrincipalsRequest {
                workspace_binding_id: 1,
            },
        )
        .await
        .unwrap();
    assert_eq!(result.state, StopWorkspacePrincipalState::Unsupported);
    assert!(
        service
            .shell
            .snapshot_sessions()
            .await
            .unwrap()
            .iter()
            .any(|s| s.session_id == id)
    );
    service
        .cancel_session(runner_protocol::CancelSessionRequest {
            owner: target,
            session_key: Some("p".into()),
            session_id: id,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn binding_cleanup_fences_other_principals_in_binding_without_blocking_other_binding() {
    let temp = tempfile::tempdir().unwrap();
    let (service, backend) = setup(temp.path());
    let cleaning = {
        let service = service.clone();
        tokio::spawn(async move {
            service
                .cleanup_workspace_binding_principals(
                    runner_protocol::CleanupWorkspaceBindingPrincipalsRequest {
                        workspace_binding_id: 1,
                    },
                )
                .await
                .unwrap()
        })
    };
    tokio::time::timeout(Duration::from_secs(1), backend.entered.notified())
        .await
        .unwrap();
    let waiting_exec = {
        let service = service.clone();
        let req = exec(owner(1), temp.path());
        tokio::spawn(async move { service.exec_shell(req).await })
    };
    let waiting_stop = {
        let service = service.clone();
        tokio::spawn(async move { service.stop_workspace_principal(request(owner(1))).await })
    };
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(!waiting_exec.is_finished());
    assert!(!waiting_stop.is_finished());
    tokio::time::timeout(
        Duration::from_secs(2),
        service.exec_shell(exec(owner(2), temp.path())),
    )
    .await
    .unwrap()
    .unwrap();
    backend.resume.notify_one();
    assert_eq!(
        cleaning.await.unwrap().state,
        StopWorkspacePrincipalState::Stopped
    );
    tokio::time::timeout(Duration::from_secs(2), waiting_exec)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), backend.entered.notified())
        .await
        .unwrap();
    backend.resume.notify_one();
    tokio::time::timeout(Duration::from_secs(2), waiting_stop)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn stop_fences_real_exec_for_its_owner_only() {
    let temp = tempfile::tempdir().unwrap();
    let (service, backend) = setup(temp.path());
    let target = owner(1);
    let stopping = {
        let service = service.clone();
        let target = target.clone();
        tokio::spawn(async move {
            service
                .stop_workspace_principal(request(target))
                .await
                .unwrap()
        })
    };
    tokio::time::timeout(Duration::from_secs(1), backend.entered.notified())
        .await
        .unwrap();
    let blocked = {
        let service = service.clone();
        let req = exec(target, temp.path());
        tokio::spawn(async move { service.exec_shell(req).await })
    };
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(!blocked.is_finished());
    tokio::time::timeout(
        Duration::from_secs(2),
        service.exec_shell(exec(owner(2), temp.path())),
    )
    .await
    .unwrap()
    .unwrap();
    backend.resume.notify_one();
    assert_eq!(
        stopping.await.unwrap().state,
        StopWorkspacePrincipalState::Stopped
    );
    tokio::time::timeout(Duration::from_secs(2), blocked)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn failed_cancellation_retains_session_and_never_reaches_backend() {
    let temp = tempfile::tempdir().unwrap();
    let (service, backend) = setup(temp.path());
    let target = owner(1);
    let mut running = exec(target.clone(), temp.path());
    running.cmd = "sleep 30".into();
    running.yield_time_ms = Some(1);
    let id = service
        .exec_shell(running)
        .await
        .unwrap()
        .session_id
        .unwrap();
    service.shell.fail_next_cancel_for_test();
    let result = service
        .stop_workspace_principal(request(target.clone()))
        .await
        .unwrap();
    assert_eq!(result.state, StopWorkspacePrincipalState::Failed);
    assert!(
        service
            .shell
            .snapshot_sessions()
            .await
            .unwrap()
            .iter()
            .any(|s| s.session_id == id)
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), backend.entered.notified())
            .await
            .is_err()
    );
    service
        .cancel_session(runner_protocol::CancelSessionRequest {
            owner: target,
            session_key: Some("p".into()),
            session_id: id,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn expired_session_racing_cancel_cannot_report_stopped() {
    let temp = tempfile::tempdir().unwrap();
    let (service, backend) = setup(temp.path());
    let target = owner(1);
    let mut running = exec(target.clone(), temp.path());
    running.cmd = "sleep 30".into();
    running.yield_time_ms = Some(1);
    service
        .exec_shell(running)
        .await
        .unwrap()
        .session_id
        .unwrap();
    service.shell.expire_before_cancel_for_test().await;
    let result = service
        .stop_workspace_principal(request(target))
        .await
        .unwrap();
    assert_eq!(result.state, StopWorkspacePrincipalState::Failed);
    assert!(service.shell.snapshot_sessions().await.unwrap().is_empty());
    assert!(
        tokio::time::timeout(Duration::from_millis(20), backend.entered.notified())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn stop_returns_busy_after_thirty_second_owner_fence_timeout() {
    let temp = tempfile::tempdir().unwrap();
    let (service, _) = setup(temp.path());
    let target = owner(1);
    let fence = service.fences.for_owner(&target);
    let _read = fence.read().await;
    let before = std::time::Instant::now();
    let result = service
        .stop_workspace_principal(request(target))
        .await
        .unwrap();
    assert_eq!(result.state, StopWorkspacePrincipalState::Busy);
    assert!(before.elapsed() >= Duration::from_secs(30));
}
