//! Shared fake-Docker harness for environment tests (issue 739 P1).
//!
//! Each fake bakes its log path into the script so parallel tests never
//! share process-global env. Stateful variants dispatch on `$1` (`ps`,
//! `inspect`, `volume`, ...) and read canned outputs from temp files, so
//! no live daemon is ever touched.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::ExecutionEnvironmentProfile;
use super::profile::command::TrustedCommand;
use super::profile::workdir::TrustedStateWorkdir;
use crate::config::{RunnerBackendKind, RunnerManagerConfig};
use crate::runtime::docker::DockerRunnerBackend;

pub fn test_manager_config(docker_cli: &str) -> RunnerManagerConfig {
    RunnerManagerConfig {
        control_plane_url: None,
        manager_id: "test-manager".to_string(),
        bind_addr: "127.0.0.1:0".to_string(),
        auth_token: "test-token".to_string(),
        backend: RunnerBackendKind::Docker,
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
        docker_cli: docker_cli.to_string(),
        docker_host: None,
        runtime_class: None,
        native_task_root: None,
        native_task_host_root: None,
    }
}

pub fn backend_with_cli(docker_cli: &str) -> Arc<DockerRunnerBackend> {
    let config = Arc::new(tokio::sync::RwLock::new(test_manager_config(docker_cli)));
    DockerRunnerBackend::new(
        config,
        "test-manager".to_string(),
        crate::runtime::RunnerLifecycleReporter::noop(),
        crate::runtime::NativeTaskStore::with_uid_source(
            None,
            crate::runtime::execution_environment::service::host_identity::UidSource::Fixed(1000),
        ),
    )
}

pub fn write_executable(dir: &Path, name: &str, script: &str) -> PathBuf {
    let path = dir.join(name);
    write_via_shell(&path, script);
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

/// Write through a short-lived `/bin/sh`, so the test process never holds a
/// write descriptor on the executed path. Under parallel tests a bare
/// `std::fs::write` fd can be inherited by another thread's `fork` between
/// fork and exec, which makes `execve` of that same script fail with a
/// spurious `ETXTBSY`; writing in a child process removes that window.
fn write_via_shell(path: &Path, contents: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg("cat > \"$1\"")
        .arg("df-fake-writer")
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn fake writer");
    child
        .stdin
        .take()
        .expect("fake writer stdin")
        .write_all(contents.as_bytes())
        .expect("write fake docker");
    let status = child.wait().expect("wait fake writer");
    assert!(
        status.success(),
        "fake writer failed for {}",
        path.display()
    );
}

/// Simple fake: `ok` (0), `missing` (No such container), `fail`
/// (permission denied), `hang` (sleep 2). Mirrors `docker_tests` harness.
pub fn write_simple_fake(dir: &Path, name: &str, log: &Path, mode: &str) -> PathBuf {
    let script = format!(
        "#!/bin/bash\necho \"$@\" >> \"{}\"\nmode=\"{}\"\nif [ \"$mode\" = \"hang\" ]; then\n  sleep 2\n  exit 0\nfi\nif [ \"$mode\" = \"missing\" ]; then\n  echo \"Error: No such container: $3\" >&2\n  exit 1\nfi\nif [ \"$mode\" = \"fail\" ]; then\n  echo \"permission denied\" >&2\n  exit 1\nfi\nexit 0\n",
        log.display(),
        mode
    );
    write_executable(dir, name, &script)
}

/// Deterministic selective hang: logs every invocation and sleeps 30s on
/// the given subcommand (`$1`), exiting 0 otherwise. The long sleep keeps
/// the deadline assertion deterministic without racing the short budget.
pub fn write_hang_fake(dir: &Path, name: &str, log: &Path, hang: &str) -> PathBuf {
    let script = format!(
        "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"{}\" ]; then\n  sleep 30\n  exit 0\nfi\nexit 0\n",
        log.display(),
        hang
    );
    write_executable(dir, name, &script)
}

/// Selective hang that records the fake CLI pid before sleeping, so a test
/// can prove the bounded runner reaped the command process.
pub fn write_hang_fake_with_pid(
    dir: &Path,
    name: &str,
    log: &Path,
    hang: &str,
    pid_file: &Path,
) -> PathBuf {
    let script = format!(
        "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"{}\" ]; then\n  echo $$ > \"{}\"\n  sleep 30\n  exit 0\nfi\nexit 0\n",
        log.display(),
        hang,
        pid_file.display()
    );
    write_executable(dir, name, &script)
}

/// Inspect JSON for one environment container.
pub fn inspect_json(status: &str, owner_key: &str, env_id: &str, generation: &str) -> String {
    inspect_json_with_networks(
        status,
        owner_key,
        env_id,
        generation,
        &serde_json::json!({}),
    )
}

/// Inspect JSON including `NetworkSettings.Networks` for service tests.
pub fn inspect_json_with_networks(
    status: &str,
    owner_key: &str,
    env_id: &str,
    generation: &str,
    networks: &serde_json::Value,
) -> String {
    serde_json::json!({
        "Id": "abc123",
        "State": {"Status": status},
        "Config": {"Labels": {
            "desk-foreman.owner": owner_key,
            "desk-foreman.manager": "test-manager",
            "desk-foreman.managed": "true",
            "desk-foreman.execution-environment": env_id,
            "desk-foreman.generation": generation
        }},
        "NetworkSettings": {"Networks": networks},
        "Mounts": []
    })
    .to_string()
}

pub const ENV_ID: &str = "123e4567-e89b-12d3-a456-426614174000";

pub fn env_owner(generation: i64) -> runner_protocol::RunnerOwner {
    runner_protocol::RunnerOwner::ExecutionEnvironment {
        environment_id: ENV_ID.to_string(),
        generation,
    }
}

/// Dedicated persistent state root for one test. The caller owns the
/// `TempDir`, so the root stays alive for the whole test.
pub fn state_dir(temp: &Path) -> PathBuf {
    temp.join("state")
}

/// Enabled profile with explicit process inputs but no persistent root.
/// Real deployments read uid/gid from the environment; tests pass them
/// directly to avoid global env races. The deadline is short enough to
/// bound the deterministic hang fakes while staying far above any
/// fake-CLI spawn latency.
pub fn bare_enabled_profile() -> ExecutionEnvironmentProfile {
    ExecutionEnvironmentProfile {
        enabled: true,
        stop_timeout: Duration::from_millis(500),
        operation_deadline: Duration::from_secs(1),
        volume_uid: Some(1000),
        volume_gid: Some(1000),
        command: TrustedCommand::Configured(vec![
            "/usr/bin/sleep".to_string(),
            "infinity".to_string(),
        ]),
        state_workdir: TrustedStateWorkdir::Configured("/state/work".to_string()),
        // A bare unset root must never silently fall back to ephemeral
        // state; tests that reach the filesystem add one explicitly.
        state_dir: None,
        ..ExecutionEnvironmentProfile::default()
    }
}

/// Enabled profile with an explicit, living state root.
pub fn enabled_profile(state_root: &Path) -> ExecutionEnvironmentProfile {
    ExecutionEnvironmentProfile {
        state_dir: Some(state_root.to_path_buf()),
        ..bare_enabled_profile()
    }
}

/// Disabled profile that still owns an explicit state root, so a
/// cleanup-only stop can pass the persistent-root precondition.
pub fn disabled_profile(state_root: &Path) -> ExecutionEnvironmentProfile {
    ExecutionEnvironmentProfile {
        state_dir: Some(state_root.to_path_buf()),
        ..ExecutionEnvironmentProfile::default()
    }
}

fn env_dir_name() -> String {
    ENV_ID.chars().filter(|c| *c != '-').collect()
}

pub fn barrier_path(state_root: &Path) -> PathBuf {
    state_root.join(env_dir_name()).join("barrier.json")
}

pub fn lock_path(state_root: &Path) -> PathBuf {
    state_root.join(env_dir_name()).join("lock")
}

/// Read the durable barrier, or `None` when it does not exist yet.
pub fn read_barrier(state_root: &Path) -> Option<serde_json::Value> {
    let raw = std::fs::read_to_string(barrier_path(state_root)).ok()?;
    Some(serde_json::from_str(&raw).expect("barrier json"))
}

/// Fake that drives a full creation: `volume`/`ps` succeed (empty ps) and
/// `inspect` reports the requested owner `running`.
pub fn write_creation_fake(
    dir: &Path,
    name: &str,
    log: &Path,
    owner_key: &str,
    generation: i64,
) -> PathBuf {
    let json_file = dir.join(format!("{name}.running.json"));
    std::fs::write(
        &json_file,
        inspect_json("running", owner_key, ENV_ID, &generation.to_string()),
    )
    .expect("running json");
    let script = format!(
        "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"ps\" ]; then\n  exit 0\nfi\nif [ \"$1\" = \"inspect\" ]; then\n  cat \"{}\"\n  exit 0\nfi\nexit 0\n",
        log.display(),
        json_file.display()
    );
    write_executable(dir, name, &script)
}

/// Fake that drives a running-to-stopped stop: `inspect` reports the owner
/// `running` until `stop` touches the marker, after which it reports
/// `exited`; `rm` succeeds.
pub fn write_stop_fake(
    dir: &Path,
    name: &str,
    log: &Path,
    owner_key: &str,
    generation: i64,
) -> PathBuf {
    let running_file = dir.join(format!("{name}.running.json"));
    let stopped_file = dir.join(format!("{name}.stopped.json"));
    let marker = dir.join(format!("{name}.stopped.marker"));
    std::fs::write(
        &running_file,
        inspect_json("running", owner_key, ENV_ID, &generation.to_string()),
    )
    .expect("running json");
    std::fs::write(
        &stopped_file,
        inspect_json("exited", owner_key, ENV_ID, &generation.to_string()),
    )
    .expect("stopped json");
    let script = format!(
        "#!/bin/bash\necho \"$@\" >> \"{}\"\nif [ \"$1\" = \"inspect\" ]; then\n  if [ -f \"{}\" ]; then\n    cat \"{}\"\n    exit 0\n  fi\n  cat \"{}\"\n  exit 0\nfi\nif [ \"$1\" = \"stop\" ]; then\n  touch \"{}\"\n  exit 0\nfi\nexit 0\n",
        log.display(),
        marker.display(),
        stopped_file.display(),
        running_file.display(),
        marker.display()
    );
    write_executable(dir, name, &script)
}
