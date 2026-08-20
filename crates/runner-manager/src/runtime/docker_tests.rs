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
