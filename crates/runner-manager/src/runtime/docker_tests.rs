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
        native_task_root: None,
        native_task_host_root: None,
    }
}

fn backend() -> Arc<DockerRunnerBackend> {
    let config: Arc<RwLock<RunnerManagerConfig>> = Arc::new(RwLock::new(test_config()));
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

fn backend_with_docker_cli(docker_cli: &str) -> Arc<DockerRunnerBackend> {
    let mut config = test_config();
    config.docker_cli = docker_cli.to_string();
    let config: Arc<RwLock<RunnerManagerConfig>> = Arc::new(RwLock::new(config));
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

/// Fake `docker` CLI that never touches live containers.
///
/// Each fake bakes its log path and mode into the script so parallel
/// tests never share process-global env: `ok` (0), `missing` (1 with
/// "No such container" on stderr, quiescent), `fail` (1 with unrelated
/// stderr, fail closed), `hang` (sleep 2, bounded by the caller timeout).
///
/// The script opens with an exact readiness argument that exits 0 before it
/// logs anything or looks at its mode. That is the only added line: for
/// `stop`, `rm`, `missing`, `fail` and `hang` the script is byte-equivalent to
/// the previous version. The readiness probe exists because a freshly written
/// fixture can still be held open by a writer when the manager first tries to
/// execute it, and the kernel then refuses the exec with `ETXTBSY`.
fn write_fake_docker(
    dir: &std::path::Path,
    name: &str,
    log: &std::path::Path,
    mode: &str,
) -> std::path::PathBuf {
    let path = dir.join(name);
    let script = format!(
        "#!/bin/bash\nif [ \"$1\" = \"{READY_ARG}\" ]; then exit 0; fi\necho \"$@\" >> \"{}\"\nmode=\"{}\"\nif [ \"$mode\" = \"hang\" ]; then\n  sleep 2\n  exit 0\nfi\nif [ \"$mode\" = \"missing\" ]; then\n  echo \"Error: No such container: $3\" >&2\n  exit 1\nfi\nif [ \"$mode\" = \"fail\" ]; then\n  echo \"permission denied\" >&2\n  exit 1\nfi\nexit 0\n",
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
    // The fixture is only usable once its published path can really be
    // executed, so the helper proves that before returning it. The script is
    // never rewritten after a successful probe.
    probe_fixture_readiness(&path, spawn_fixture_readiness_probe)
        .expect("published fake docker must be executable before any test uses it");
    path
}

/// Exact argument that makes a published fake CLI report readiness and exit.
const READY_ARG: &str = "__df_fixture_ready";
/// `ETXTBSY`: the errno returned while a writer still holds the script open.
const FIXTURE_BUSY_ERRNO: i32 = 26;
/// Upper bound on readiness probes, so a stuck fixture fails instead of hanging.
const FIXTURE_READY_MAX_ATTEMPTS: u32 = 64;
/// Aggregate budget for the whole readiness wait.
const FIXTURE_READY_BUDGET: std::time::Duration = std::time::Duration::from_secs(1);
/// Bounded pause between readiness attempts.
const FIXTURE_READY_BACKOFF: std::time::Duration = std::time::Duration::from_millis(1);

/// Wait until a published fixture can actually be executed.
///
/// Only `ETXTBSY` is retried, only here, and only while the bounded budget
/// lasts; any other spawn error, a non-zero exit or unexpected output fails
/// immediately so a real defect is never masked. The gate is a real successful
/// spawn, exit and reap — never elapsed time.
fn probe_fixture_readiness<F>(path: &std::path::Path, mut probe: F) -> anyhow::Result<()>
where
    F: FnMut(&std::path::Path, &str) -> std::io::Result<std::process::Output>,
{
    let deadline = std::time::Instant::now() + FIXTURE_READY_BUDGET;
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match probe(path, READY_ARG) {
            Ok(output) => {
                if !output.status.success() {
                    anyhow::bail!(
                        "fake docker readiness probe for {} exited with {:?} on attempt {attempt}",
                        path.display(),
                        output.status
                    );
                }
                let stdout = String::from_utf8_lossy(&output.stdout);
                if !stdout.trim().is_empty() {
                    anyhow::bail!(
                        "fake docker readiness probe for {} produced unexpected output {:?} on attempt {attempt}",
                        path.display(),
                        stdout.trim()
                    );
                }
                return Ok(());
            }
            Err(error) if error.raw_os_error() == Some(FIXTURE_BUSY_ERRNO) => {
                if attempt >= FIXTURE_READY_MAX_ATTEMPTS || std::time::Instant::now() >= deadline {
                    anyhow::bail!(
                        "fake docker readiness probe for {} was still busy after {attempt} attempts within {FIXTURE_READY_BUDGET:?}: {error}",
                        path.display()
                    );
                }
                std::thread::sleep(FIXTURE_READY_BACKOFF);
            }
            Err(error) => {
                anyhow::bail!(
                    "fake docker readiness probe for {} failed on attempt {attempt}: {error}",
                    path.display()
                );
            }
        }
    }
}

fn spawn_fixture_readiness_probe(
    path: &std::path::Path,
    arg: &str,
) -> std::io::Result<std::process::Output> {
    std::process::Command::new(path).arg(arg).output()
}

#[tokio::test]
async fn a_spawn_failure_names_the_os_class_without_leaking_the_arguments() {
    // No real Docker: the configured CLI path simply does not exist, so
    // `Command::output()` fails at spawn.
    let backend = backend_with_docker_cli("/nonexistent/docker-cli-for-822");
    let error = backend
        .docker_output(["stop", "df-secret-container-name"])
        .await
        .expect_err("spawning a missing CLI must fail");
    let chain = format!("{error:#}");

    assert!(
        chain.starts_with("failed to execute /nonexistent/docker-cli-for-822:"),
        "unexpected spawn context: {chain}"
    );
    // The added classification is a bounded `io::ErrorKind` plus the numeric
    // OS code. Its exact wording is Rust's, so the test asserts the shape
    // rather than an English string.
    let mut segments = chain.splitn(3, ": ");
    assert_eq!(
        segments.next(),
        Some("failed to execute /nonexistent/docker-cli-for-822")
    );
    let classification = segments.next().unwrap_or_default();
    assert!(
        !classification.is_empty() && classification.ends_with("(os error 2)"),
        "the bounded OS classification must be present: {chain}"
    );
    assert!(
        segments.next().is_some(),
        "the original io error stays in the chain as the source: {chain}"
    );
    assert!(
        !chain.contains("df-secret-container-name") && !chain.contains("stop"),
        "a spawn failure must not echo the argv: {chain}"
    );
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

#[cfg(unix)]
fn ready_output(stdout: &[u8], code: i32) -> std::process::Output {
    use std::os::unix::process::ExitStatusExt;
    std::process::Output {
        status: std::process::ExitStatus::from_raw(code),
        stdout: stdout.to_vec(),
        stderr: Vec::new(),
    }
}

#[test]
fn fixture_readiness_retries_only_while_the_script_is_busy() {
    let path = std::path::Path::new("/tmp/docker-fixture-readiness");
    let mut attempts = 0u32;
    let outcome = probe_fixture_readiness(path, |_, arg| {
        attempts += 1;
        assert_eq!(
            arg, READY_ARG,
            "the probe uses the exact readiness argument"
        );
        if attempts < 3 {
            Err(std::io::Error::from_raw_os_error(FIXTURE_BUSY_ERRNO))
        } else {
            Ok(ready_output(b"", 0))
        }
    });

    assert!(outcome.is_ok(), "ETXTBSY must clear: {outcome:?}");
    assert_eq!(attempts, 3, "exactly one retry per busy attempt");
}

#[test]
fn fixture_readiness_never_retries_another_errno() {
    let path = std::path::Path::new("/tmp/docker-fixture-readiness");
    let mut attempts = 0u32;
    let outcome = probe_fixture_readiness(path, |_, _| {
        attempts += 1;
        Err(std::io::Error::from_raw_os_error(13)) // EACCES
    });

    let message = outcome
        .expect_err("a denied spawn must fail the fixture")
        .to_string();
    assert_eq!(attempts, 1, "only ETXTBSY may be retried");
    assert!(
        message.contains("failed on attempt 1") && message.contains("Permission denied"),
        "the real error must be reported, not retried away: {message}"
    );
}

#[test]
fn fixture_readiness_gives_up_within_its_bounded_budget() {
    let path = std::path::Path::new("/tmp/docker-fixture-readiness");
    let mut attempts = 0u32;
    let started = std::time::Instant::now();
    let outcome = probe_fixture_readiness(path, |_, _| {
        attempts += 1;
        Err(std::io::Error::from_raw_os_error(FIXTURE_BUSY_ERRNO))
    });
    let elapsed = started.elapsed();

    let message = outcome
        .expect_err("a permanently busy fixture must fail")
        .to_string();
    assert!(
        attempts <= FIXTURE_READY_MAX_ATTEMPTS,
        "attempts must stay bounded: {attempts}"
    );
    assert!(attempts > 1, "the budget must actually be used");
    assert!(
        elapsed <= FIXTURE_READY_BUDGET + std::time::Duration::from_secs(1),
        "the readiness wait must stay bounded, took {elapsed:?}"
    );
    assert!(
        message.contains("was still busy") && message.contains(&attempts.to_string()),
        "the failure must report what was measured: {message}"
    );
}

#[test]
fn fixture_readiness_refuses_a_non_zero_exit_or_any_output() {
    let path = std::path::Path::new("/tmp/docker-fixture-readiness");

    let failed_exit = probe_fixture_readiness(path, |_, _| Ok(ready_output(b"", 3)))
        .expect_err("a non-zero readiness exit is a broken fixture");
    assert!(
        failed_exit.to_string().contains("exited with"),
        "{failed_exit}"
    );

    let unexpected = probe_fixture_readiness(path, |_, _| Ok(ready_output(b"noise\n", 0)))
        .expect_err("a readiness probe must be silent");
    assert!(
        unexpected.to_string().contains("unexpected output"),
        "{unexpected}"
    );
}

#[cfg(unix)]
#[test]
fn a_published_fake_cli_is_ready_without_logging_the_probe_argument() {
    let temp = tempfile::tempdir().expect("tempdir");
    let log = temp.path().join("docker-ready.log");
    std::fs::write(&log, "").expect("seed log");
    // The helper has already probed readiness by the time it returns here.
    let fake = write_fake_docker(temp.path(), "docker-ready", &log, "ok");

    let after_probe = std::fs::read_to_string(&log).expect("read log");
    assert!(
        !after_probe.contains(READY_ARG) && after_probe.is_empty(),
        "the readiness argument must never reach the argv log: {after_probe}"
    );

    // The published fixture is a real, reaped executable.
    let ready = std::process::Command::new(&fake)
        .arg(READY_ARG)
        .output()
        .expect("run the readiness argument");
    assert!(ready.status.success() && ready.stdout.is_empty());
    assert!(
        std::fs::read_to_string(&log).expect("read log").is_empty(),
        "the readiness branch must exit before logging"
    );

    // Normal behaviour is unchanged: stop and rm still append their argv.
    for verb in ["stop", "rm"] {
        let output = std::process::Command::new(&fake)
            .arg(verb)
            .arg("desk-foreman-runner-workspace-binding-42")
            .output()
            .expect("run the fake docker");
        assert!(output.status.success(), "{verb} must still exit 0");
    }
    let logged = std::fs::read_to_string(&log).expect("read log");
    assert!(
        logged.contains("stop desk-foreman-runner-workspace-binding-42"),
        "{logged}"
    );
    assert!(
        logged.contains("rm desk-foreman-runner-workspace-binding-42"),
        "{logged}"
    );
    assert!(!logged.contains(READY_ARG), "{logged}");
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

#[tokio::test]
async fn legacy_ensure_rejects_execution_environment_owner_before_docker() {
    let temp = tempfile::tempdir().expect("tempdir");
    let log = temp.path().join("docker-env-owner.log");
    std::fs::write(&log, "").expect("seed log");
    let fake = write_fake_docker(temp.path(), "docker-env-owner", &log, "ok");
    let backend = backend_with_docker_cli(&fake.to_string_lossy());
    let owner = RunnerOwner::ExecutionEnvironment {
        environment_id: "123e4567-e89b-12d3-a456-426614174000".to_string(),
        generation: 0,
    };

    let result = backend
        .ensure_runner(&owner, std::path::Path::new("/tmp"), false)
        .await;

    let error = result.expect_err("environment owners must be rejected by the legacy path");
    assert!(
        error.to_string().contains("ensure_execution_environment"),
        "rejection must point at the environment primitive, got: {error}"
    );
    let logged = std::fs::read_to_string(&log).expect("read log");
    assert!(
        logged.is_empty(),
        "no Docker command may run for an environment owner, got: {logged}"
    );
}

#[tokio::test]
async fn legacy_ensure_still_enters_docker_for_a_workspace_owner() {
    let temp = tempfile::tempdir().expect("tempdir");
    let log = temp.path().join("docker-legacy-owner.log");
    std::fs::write(&log, "").expect("seed log");
    let fake = write_fake_docker(temp.path(), "docker-legacy-owner", &log, "ok");
    let backend = backend_with_docker_cli(&fake.to_string_lossy());
    let owner = RunnerOwner::WorkspaceBinding {
        workspace_binding_id: 42,
    };

    // The guard is scoped to environment owners: a legacy workspace owner still
    // reaches Docker even though the fake CLI cannot complete a real ensure.
    let _ = backend
        .ensure_runner(&owner, std::path::Path::new("/tmp"), false)
        .await;

    let logged = std::fs::read_to_string(&log).expect("read log");
    assert!(
        !logged.is_empty(),
        "a workspace owner must not be blocked before Docker"
    );
}
