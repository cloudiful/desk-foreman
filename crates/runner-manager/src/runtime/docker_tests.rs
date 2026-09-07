use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use runner_protocol::RunnerOwner;
use tokio::sync::RwLock;

use super::{DockerRunnerBackend, docker_lifecycle::legacy_workspace_mount_matches};
use crate::config::RunnerManagerConfig;
use crate::runtime::backend::{RunnerBackend, RunnerOperationLease};

fn test_config() -> RunnerManagerConfig {
    RunnerManagerConfig {
        control_plane_url: None,
        manager_id: "test-manager".to_string(),
        bind_addr: "127.0.0.1:0".to_string(),
        auth_token: "test-token".to_string(),
        backend: crate::config::RunnerBackendKind::Docker,
        workspace_root: PathBuf::from("/tmp"),
        host_workspace_root: PathBuf::from("/tmp"),
        image: "test-image".to_string(),
        workdir: "/workspace".to_string(),
        network_enabled: false,
        max_output_bytes: 256,
        max_timeout_ms: 1000,
        max_sessions: 1,
        pids_limit: 64,
        memory_limit: "256m".to_string(),
        cpu_limit: "1".to_string(),
        idle_ttl: Duration::from_secs(1),
        docker_cli: "docker".to_string(),
        docker_host: None,
        runtime_class: None,
    }
}

fn backend() -> Arc<DockerRunnerBackend> {
    let config: Arc<RwLock<RunnerManagerConfig>> = Arc::new(RwLock::new(test_config()));
    DockerRunnerBackend::new(
        config,
        "test-manager".to_string(),
        crate::runtime::RunnerLifecycleReporter::noop(),
    )
}

fn backend_with_docker_cli(docker_cli: &str) -> Arc<DockerRunnerBackend> {
    let mut config = test_config();
    config.docker_cli = docker_cli.to_string();
    let config: Arc<RwLock<RunnerManagerConfig>> = Arc::new(RwLock::new(config));
    DockerRunnerBackend::new(
        config,
        "test-manager".to_string(),
        crate::runtime::RunnerLifecycleReporter::noop(),
    )
}

/// Fake `docker` CLI that never touches live containers.
///
/// Each fake bakes its log path and mode into the script so parallel
/// tests never share process-global env: `ok` (0), `missing` (1 with
/// "No such container" on stderr, quiescent), `fail` (1 with unrelated
/// stderr, fail closed), `hang` (sleep 2, bounded by the caller timeout).
fn write_fake_docker(
    dir: &std::path::Path,
    name: &str,
    log: &std::path::Path,
    mode: &str,
) -> std::path::PathBuf {
    let path = dir.join(name);
    let script = format!(
        "#!/bin/bash\necho \"$@\" >> \"{}\"\nmode=\"{}\"\nif [ \"$mode\" = \"hang\" ]; then\n  sleep 2\n  exit 0\nfi\nif [ \"$mode\" = \"missing\" ]; then\n  echo \"Error: No such container: $3\" >&2\n  exit 1\nfi\nif [ \"$mode\" = \"fail\" ]; then\n  echo \"permission denied\" >&2\n  exit 1\nfi\nexit 0\n",
        log.display(),
        mode
    );
    std::fs::write(&path, script).expect("write fake docker");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path)
            .expect("stat fake docker")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod fake docker");
    }
    path
}

#[test]
fn active_op_counter_blocks_janitor_removal() {
    let backend = backend();
    let owner = RunnerOwner::InternalUser { user_id: 7 };
    let key = owner.stable_key();
    backend.bump_active(&key);
    assert!(backend.has_active_operation(&key));
    backend.decrement_active(&key);
    assert!(!backend.has_active_operation(&key));
}

#[test]
fn active_operation_lookup_works_for_another_owner() {
    let backend = backend();
    let owner = RunnerOwner::InternalUser { user_id: 42 };
    let key = owner.stable_key();
    backend.bump_active(&key);
    assert!(backend.has_active_operation(&key));
    backend.decrement_active(&key);
    assert!(!backend.has_active_operation(&key));
}

#[test]
fn operation_lease_releases_active_owner_on_drop() {
    let backend = backend();
    let owner = RunnerOwner::InternalUser { user_id: 7 };
    let backend_trait: Arc<dyn RunnerBackend> = backend.clone();
    {
        let _lease = RunnerOperationLease::new(backend_trait, owner.clone());
        assert!(backend.has_active_operation(&owner.stable_key()));
    }
    assert!(!backend.has_active_operation(&owner.stable_key()));
}

#[test]
fn legacy_workspace_mount_must_stay_under_manager_root() {
    let config = test_config();
    assert!(legacy_workspace_mount_matches(
        &config,
        Some(std::path::Path::new("/tmp/workspace/users/1"))
    ));
    assert!(!legacy_workspace_mount_matches(
        &config,
        Some(std::path::Path::new("/other-manager/workspace"))
    ));
}

#[tokio::test]
async fn takeover_terminate_removes_task_container_and_preserves_workspace() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace_file = temp.path().join("keep.txt");
    std::fs::write(&workspace_file, "user data").expect("seed workspace file");
    let log = temp.path().join("docker-ok.log");
    std::fs::write(&log, "").expect("seed log");
    // Fake CLI only appends to a temp log; no live containers touched.
    let fake = write_fake_docker(temp.path(), "docker-ok", &log, "ok");
    let backend = backend_with_docker_cli(&fake.to_string_lossy());
    let container = "desk-foreman-runner-workspace-binding-42";
    backend
        .terminate_container_for_takeover(container)
        .await
        .expect("fake stop+rm should succeed");
    let logged = std::fs::read_to_string(&log).expect("read log");
    assert!(
        logged.contains("stop") && logged.contains(container),
        "must docker stop the task container, got: {logged}"
    );
    assert!(
        logged.contains("rm") && logged.contains(container),
        "must docker rm the task container, got: {logged}"
    );
    // Workspace bind-mount content must survive container removal.
    assert_eq!(
        std::fs::read_to_string(&workspace_file).expect("read workspace"),
        "user data"
    );
}

#[tokio::test]
async fn takeover_terminate_missing_container_is_quiescent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let log = temp.path().join("docker-missing.log");
    std::fs::write(&log, "").expect("seed log");
    let fake = write_fake_docker(temp.path(), "docker-missing", &log, "missing");
    let backend = backend_with_docker_cli(&fake.to_string_lossy());
    backend
        .terminate_container_for_takeover("desk-foreman-runner-workspace-binding-99")
        .await
        .expect("missing container must map to Ok (already quiescent)");
}

#[tokio::test]
async fn takeover_terminate_daemon_failure_fails_closed() {
    let temp = tempfile::tempdir().expect("tempdir");
    let log = temp.path().join("docker-fail.log");
    std::fs::write(&log, "").expect("seed log");
    let fake = write_fake_docker(temp.path(), "docker-fail", &log, "fail");
    let backend = backend_with_docker_cli(&fake.to_string_lossy());
    let result = backend
        .terminate_container_for_takeover("desk-foreman-runner-workspace-binding-42")
        .await;
    assert!(
        result.is_err(),
        "daemon failure must fail closed, not report quiescence"
    );
}

#[tokio::test]
async fn takeover_terminate_hanging_daemon_is_bounded() {
    let temp = tempfile::tempdir().expect("tempdir");
    let log = temp.path().join("docker-hang.log");
    std::fs::write(&log, "").expect("seed log");
    let fake = write_fake_docker(temp.path(), "docker-hang", &log, "hang");
    let backend = backend_with_docker_cli(&fake.to_string_lossy());
    let result = backend
        .terminate_container_for_takeover_with_timeout(
            "desk-foreman-runner-workspace-binding-42",
            std::time::Duration::from_millis(80),
        )
        .await;
    assert!(
        result.is_err() && result.unwrap_err().to_string().contains("timed out"),
        "hanging daemon must time out instead of pinning takeover"
    );
}

#[tokio::test]
async fn cleanup_blocks_while_owner_has_active_operations() {
    use crate::runtime::backend::RunnerBackend;
    let temp = tempfile::tempdir().expect("tempdir");
    let log = temp.path().join("docker-blocked.log");
    std::fs::write(&log, "").expect("seed log");
    let fake = write_fake_docker(temp.path(), "docker-blocked", &log, "ok");
    let backend = backend_with_docker_cli(&fake.to_string_lossy());
    let owner = RunnerOwner::WorkspaceBinding {
        workspace_binding_id: 42,
    };
    backend.bump_active(&owner.stable_key());
    let result = backend.cleanup_runner_owner(owner).await;
    assert!(
        result.is_err(),
        "active operations must block container removal (retry safety)"
    );
    let logged = std::fs::read_to_string(&log).expect("read log");
    assert!(
        !logged.contains("stop"),
        "blocked cleanup must not docker stop, got: {logged}"
    );
}
