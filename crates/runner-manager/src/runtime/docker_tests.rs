use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use runner_protocol::RunnerOwner;
use tokio::sync::RwLock;

use super::{DockerRunnerBackend, docker_lifecycle::legacy_workspace_mount_matches};
use crate::config::RunnerManagerConfig;
use crate::runtime::backend::{RunnerBackend, RunnerOperationLease};

pub(super) fn test_config() -> RunnerManagerConfig {
    RunnerManagerConfig {
        control_plane_url: None,
        manager_id: "test-manager".to_string(),
        bind_addr: "127.0.0.1:0".to_string(),
        auth_token: "test-token".to_string(),
        backend: crate::config::RunnerBackendKind::Docker,
        workspace_root: PathBuf::from("/tmp"),
        host_workspace_root: PathBuf::from("/tmp"),
        image: "test-image".to_string(),
        git_image: None,
        git_private_root: None,
        git_host_private_root: None,
        git_network: None,
        git_broker_host: None,
        git_max_fetch_volume_bytes: None,
        git_max_bundle_bytes: None,
        git_max_logical_object_bytes: None,
        git_max_checkout_bytes: None,
        git_max_tree_entries: None,
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
pub(super) fn probe_fixture_readiness<F>(path: &std::path::Path, mut probe: F) -> anyhow::Result<()>
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

struct PrincipalTestBackend {
    inner: Arc<super::DirectRunnerBackend>,
    stops: std::sync::atomic::AtomicUsize,
}
impl RunnerBackend for PrincipalTestBackend {
    fn reconcile<'a>(&'a self) -> desk_foreman::runner::RunnerFuture<'a, anyhow::Result<()>> {
        self.inner.reconcile()
    }
    fn prepare_shell_spawn<'a>(
        &'a self,
        request: runner_protocol::RunnerShellRequest,
    ) -> desk_foreman::runner::RunnerFuture<'a, anyhow::Result<super::ProcessSpawnTarget>> {
        self.inner.prepare_shell_spawn(request)
    }
    fn run_command<'a>(
        &'a self,
        request: runner_protocol::RunnerCommandRequest,
    ) -> desk_foreman::runner::RunnerFuture<'a, anyhow::Result<runner_protocol::CommandOutput>>
    {
        self.inner.run_command(request)
    }
    fn cleanup_runner_owner<'a>(
        &'a self,
        owner: RunnerOwner,
    ) -> desk_foreman::runner::RunnerFuture<'a, anyhow::Result<()>> {
        self.inner.cleanup_runner_owner(owner)
    }
    fn supports_workspace_principal_stop(&self) -> bool {
        true
    }
    fn stop_workspace_principal<'a>(
        &'a self,
        _: runner_protocol::StopWorkspacePrincipalRequest,
    ) -> desk_foreman::runner::RunnerFuture<
        'a,
        anyhow::Result<runner_protocol::StopWorkspacePrincipalAck>,
    > {
        self.stops.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async {
            Ok(runner_protocol::StopWorkspacePrincipalAck {
                state: runner_protocol::StopWorkspacePrincipalState::Stopped,
                detail: None,
            })
        })
    }
}

#[tokio::test]
async fn principal_direct_stop_leaves_sessions_untouched() {
    use desk_foreman::runner::RunnerService;
    use runner_protocol::{
        ExecRequest, StopWorkspacePrincipalRequest, StopWorkspacePrincipalState as State,
    };
    let temp = tempfile::tempdir().unwrap();
    let mut config = test_config();
    config.workspace_root = temp.path().into();
    config.host_workspace_root = temp.path().into();
    let service = super::LocalRunnerService::new(
        super::DirectRunnerBackend::new(),
        Arc::new(RwLock::new(config)),
    );
    let owner =
        runner_protocol::workspace_principal::application_principal_owner(1, 2, "u", "w").unwrap();
    let session_id = service
        .exec_shell(ExecRequest {
            owner: RunnerOwner::InternalUser { user_id: 1 },
            session_key: Some("p".into()),
            workspace_root: temp.path().into(),
            cmd: "sleep 30".into(),
            workdir: None,
            shell: "sh".into(),
            login: false,
            tty: false,
            timeout_ms: None,
            yield_time_ms: Some(1),
            max_output_tokens: None,
            max_output_bytes: None,
            network_enabled: false,
        })
        .await
        .unwrap()
        .session_id
        .unwrap();
    assert_eq!(
        service
            .stop_workspace_principal(StopWorkspacePrincipalRequest {
                owner: owner.clone(),
                principal_id: "p".into()
            })
            .await
            .unwrap()
            .state,
        State::Unsupported
    );
    assert!(
        service
            .list_sessions()
            .await
            .unwrap()
            .iter()
            .any(|s| s.session_id == session_id)
    );
    service
        .cancel_session(runner_protocol::CancelSessionRequest {
            owner: RunnerOwner::InternalUser { user_id: 1 },
            session_key: Some("p".into()),
            session_id,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn principal_local_stop_rejects_foreign_session_and_drains_own_only() {
    use desk_foreman::runner::RunnerService;
    use runner_protocol::{
        ExecRequest, StopWorkspacePrincipalRequest, StopWorkspacePrincipalState as State,
    };
    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(PrincipalTestBackend {
        inner: super::DirectRunnerBackend::new(),
        stops: std::sync::atomic::AtomicUsize::new(0),
    });
    let mut config = test_config();
    config.workspace_root = temp.path().into();
    config.host_workspace_root = temp.path().into();
    config.max_sessions = 4;
    let service = super::LocalRunnerService::new(backend.clone(), Arc::new(RwLock::new(config)));
    let owner =
        runner_protocol::workspace_principal::application_principal_owner(1, 2, "u", "w").unwrap();
    let other =
        runner_protocol::workspace_principal::application_principal_owner(1, 3, "u", "w").unwrap();
    let spawn = |owner: RunnerOwner, session_key: Option<String>| ExecRequest {
        owner,
        session_key,
        workspace_root: temp.path().into(),
        cmd: "sleep 30".into(),
        workdir: None,
        shell: "sh".into(),
        login: false,
        tty: false,
        timeout_ms: None,
        yield_time_ms: Some(1),
        max_output_tokens: None,
        max_output_bytes: None,
        network_enabled: false,
    };
    let foreign = service
        .exec_shell(spawn(owner.clone(), Some("foreign".into())))
        .await
        .unwrap()
        .session_id
        .unwrap();
    let other_id = service
        .exec_shell(spawn(other.clone(), Some("other".into())))
        .await
        .unwrap()
        .session_id
        .unwrap();
    let request = StopWorkspacePrincipalRequest {
        owner: owner.clone(),
        principal_id: "p".into(),
    };
    assert_eq!(
        service
            .stop_workspace_principal(request.clone())
            .await
            .unwrap()
            .state,
        State::Conflict
    );
    assert_eq!(backend.stops.load(std::sync::atomic::Ordering::SeqCst), 0);
    service
        .cancel_session(runner_protocol::CancelSessionRequest {
            owner: owner.clone(),
            session_key: Some("foreign".into()),
            session_id: foreign,
        })
        .await
        .unwrap();
    let unknown = service
        .exec_shell(spawn(owner.clone(), None))
        .await
        .unwrap()
        .session_id
        .unwrap();
    assert_eq!(
        service
            .stop_workspace_principal(request.clone())
            .await
            .unwrap()
            .state,
        State::Conflict
    );
    assert_eq!(backend.stops.load(std::sync::atomic::Ordering::SeqCst), 0);
    service
        .cancel_session(runner_protocol::CancelSessionRequest {
            owner: owner.clone(),
            session_key: None,
            session_id: unknown,
        })
        .await
        .unwrap();
    service
        .exec_shell(spawn(owner.clone(), Some("p".into())))
        .await
        .unwrap()
        .session_id
        .unwrap();
    assert_eq!(
        service
            .stop_workspace_principal(request)
            .await
            .unwrap()
            .state,
        State::Stopped
    );
    assert_eq!(backend.stops.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        service
            .list_sessions()
            .await
            .unwrap()
            .iter()
            .any(|s| s.session_id == other_id)
    );
    service
        .cancel_session(runner_protocol::CancelSessionRequest {
            owner: other,
            session_key: Some("other".into()),
            session_id: other_id,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn binding_cleanup_discovers_unreported_containers_and_fails_closed() {
    use runner_protocol::{
        CleanupWorkspaceBindingPrincipalsRequest, StopWorkspacePrincipalState as State,
    };
    use std::os::unix::fs::PermissionsExt;
    let first =
        runner_protocol::workspace_principal::application_principal_owner(1, 2, "u1", "w").unwrap();
    let second =
        runner_protocol::workspace_principal::application_principal_owner(1, 2, "u2", "w").unwrap();
    let other =
        runner_protocol::workspace_principal::application_principal_owner(1, 3, "u3", "w").unwrap();
    for mode in [
        "success",
        "foreign",
        "unmanaged",
        "missing_owner",
        "stripped",
        "malformed",
        "unrelated",
        "wrong_name",
        "other_binding_label",
        "residual",
        "inspect_error",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let fake = temp.path().join("docker");
        let log = temp.path().join("log");
        let marker1 = temp.path().join("one");
        let marker2 = temp.path().join("two");
        let metadata = |owner: &RunnerOwner, id: &str, manager: &str| {
            serde_json::json!({"Name":format!("/{}",owner.container_name()),"Id":id,"State":{"Status":"running"},"Config":{"Labels":{"desk-foreman.managed":"true","desk-foreman.owner":owner.stable_key(),"desk-foreman.manager":manager}}}).to_string()
        };
        let manager = if mode == "foreign" {
            "other-manager"
        } else {
            "test-manager"
        };
        let mut first_metadata: serde_json::Value =
            serde_json::from_str(&metadata(&first, "id-one", manager)).unwrap();
        if mode == "unmanaged" {
            first_metadata["Config"]["Labels"]["desk-foreman.managed"] = serde_json::json!("false");
        }
        if mode == "missing_owner" {
            first_metadata["Config"]["Labels"]
                .as_object_mut()
                .unwrap()
                .remove("desk-foreman.owner");
        }
        if mode == "wrong_name" {
            first_metadata["Name"] = serde_json::json!("/wrong");
        }
        if mode == "stripped" {
            first_metadata["Config"]["Labels"] = serde_json::json!({});
        }
        if mode == "malformed" {
            first_metadata["Config"]["Labels"]["desk-foreman.owner"] = serde_json::json!("user:9");
        }
        let first_name = if mode == "malformed" {
            "desk-foreman-runner-application_principal-1-2-bad".to_string()
        } else {
            first.container_name()
        };
        if mode == "other_binding_label" {
            first_metadata["Config"]["Labels"]["desk-foreman.owner"] =
                serde_json::json!(other.stable_key());
        }
        let script = format!(
            r##"#!/bin/bash
if [ "$1" = "{READY_ARG}" ]; then exit 0; fi
echo "$@" >> '{log}'
case "$1" in
 ps) case "$4" in
   name=*) [ -e '{m1}' ] || echo '{n1}'; [ -e '{m2}' ] || echo '{n2}'; echo '{n3}' ;;
   label=*) if [ '{mode}' != stripped ]; then [ -e '{m1}' ] || echo '{n1}'; fi; [ -e '{m2}' ] || echo '{n2}'; echo '{n3}'; echo 'legacy-user'; echo 'legacy-binding'; echo 'legacy-environment' ;;
   *) echo unrelated-service ;;
  esac ;;
 inspect)
  if [ '{mode}' = inspect_error ]; then echo 'permission denied' >&2; exit 1; fi
  case "$2" in
   '{n1}'|id-one) [ -e '{m1}' ] && {{ echo 'No such container' >&2; exit 1; }}; echo '{j1}' ;;
   '{n2}'|id-two) [ -e '{m2}' ] && {{ echo 'No such container' >&2; exit 1; }}; echo '{j2}' ;;
   '{n3}') echo '{j3}' ;;
   'legacy-user') echo '{{"Config":{{"Labels":{{"desk-foreman.owner":"user:9"}}}}}}' ;;
   'legacy-binding') echo '{{"Config":{{"Labels":{{"desk-foreman.owner":"workspace_binding:2"}}}}}}' ;;
   'legacy-environment') echo '{{"Config":{{"Labels":{{"desk-foreman.owner":"execution_environment:abc:0"}}}}}}' ;;
   *) echo 'No such container' >&2; exit 1 ;;
  esac ;;
 rm) if [ '{mode}' != residual ]; then case "$2" in id-one) touch '{m1}' ;; id-two) touch '{m2}' ;; esac; fi ;;
esac
"##,
            log = log.display(),
            m1 = marker1.display(),
            m2 = marker2.display(),
            n1 = first_name,
            n2 = second.container_name(),
            n3 = other.container_name(),
            mode = mode,
            j1 = first_metadata,
            j2 = metadata(&second, "id-two", "test-manager"),
            j3 = metadata(&other, "id-other", "test-manager")
        );
        std::fs::write(&fake, script).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        probe_fixture_readiness(&fake, spawn_fixture_readiness_probe).unwrap();
        let backend = backend_with_docker_cli(fake.to_str().unwrap());
        let result = backend
            .cleanup_workspace_binding_principals(CleanupWorkspaceBindingPrincipalsRequest {
                workspace_binding_id: 2,
            })
            .await
            .unwrap();
        let calls = std::fs::read_to_string(&log).unwrap();
        if matches!(mode, "success" | "unrelated") {
            assert_eq!(result.state, State::Stopped);
            assert_eq!(result.owners.len(), 2);
            assert!(calls.contains("rm id-one") && calls.contains("rm id-two"));
        } else {
            assert_ne!(result.state, State::Stopped, "{mode}");
        }
        assert!(!calls.contains("rm id-other"));
        assert!(!calls.contains("inspect unrelated-service"));
        assert!(calls.contains("--filter label=desk-foreman.managed"));
        assert!(calls.contains("--filter label=desk-foreman.owner"));
        assert!(calls.contains("--filter name=desk-foreman-runner-application_principal-"));
        assert!(
            !calls
                .lines()
                .any(|line| line.starts_with("stop legacy-") || line.starts_with("rm legacy-"))
        );
        if matches!(mode, "foreign" | "unmanaged" | "wrong_name") {
            assert!(!calls.contains("stop id-one"));
        }
        if matches!(
            mode,
            "missing_owner" | "stripped" | "malformed" | "other_binding_label" | "inspect_error"
        ) {
            assert!(!calls.contains("stop "));
        }
    }
}

#[tokio::test]
async fn principal_stop_validates_and_confirms_immutable_id_and_name() {
    use runner_protocol::{StopWorkspacePrincipalRequest, StopWorkspacePrincipalState as State};
    use std::os::unix::fs::PermissionsExt;
    let owner = runner_protocol::workspace_principal::application_principal_owner(
        1,
        2,
        "user",
        "workspace",
    )
    .unwrap();
    for mode in [
        "success",
        "foreign",
        "residual",
        "absent",
        "error",
        "busy",
        "missing_manager",
        "missing_managed",
        "wrong_owner",
        "wrong_name",
        "missing_id",
        "replaced",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let fake = temp.path().join("docker");
        let log = temp.path().join("log");
        let marker = temp.path().join("removed");
        let manager = if mode == "foreign" {
            "foreign"
        } else {
            "test-manager"
        };
        let metadata = serde_json::json!({"Name": if mode == "wrong_name" { "/other".to_string() } else { format!("/{}", owner.container_name()) }, "Id": if mode == "missing_id" { "" } else { "immutable-id" },"State":{"Status":"running"},"Config":{"Labels":{"desk-foreman.managed": if mode == "missing_managed" { "false" } else { "true" },"desk-foreman.owner": if mode == "wrong_owner" { "foreign".to_string() } else { owner.stable_key() },"desk-foreman.manager": if mode == "missing_manager" { "" } else { manager }}}}).to_string();
        let script = format!(
            "#!/bin/bash\nif [ \"$1\" = \"{READY_ARG}\" ]; then exit 0; fi\necho \"$@\" >> '{}'\ncase \"$1\" in\n inspect) if [ '{}' = error ]; then echo 'permission denied' >&2; exit 1; fi; if [ '{}' = absent ] || {{ [ -e '{}' ] && [ '{}' != residual ] && {{ [ '{}' != replaced ] || [ \"$2\" = immutable-id ]; }}; }}; then echo 'No such container' >&2; exit 1; fi; printf '%s\\n' '{}' ;;\n rm) touch '{}' ;;\nesac\n",
            log.display(),
            mode,
            mode,
            marker.display(),
            mode,
            mode,
            metadata,
            marker.display()
        );
        std::fs::write(&fake, script).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        probe_fixture_readiness(&fake, spawn_fixture_readiness_probe).unwrap();
        let backend = backend_with_docker_cli(fake.to_str().unwrap());
        if mode == "busy" {
            backend.bump_active(&owner.stable_key());
        }
        let result = backend
            .stop_workspace_principal(StopWorkspacePrincipalRequest {
                owner: owner.clone(),
                principal_id: "p".into(),
            })
            .await
            .unwrap();
        let expected = match mode {
            "success" | "absent" => State::Stopped,
            "foreign" | "missing_manager" | "missing_managed" | "wrong_owner" | "wrong_name"
            | "missing_id" => State::Conflict,
            "busy" => State::Busy,
            _ => State::Failed,
        };
        assert_eq!(result.state, expected, "{mode}");
        let calls = std::fs::read_to_string(&log).unwrap_or_default();
        if matches!(mode, "success" | "residual" | "replaced") {
            assert!(calls.contains("stop immutable-id"));
            assert!(calls.contains("rm immutable-id"));
            assert!(calls.contains("inspect immutable-id"));
        } else {
            assert!(!calls.contains("stop "), "{mode}: {calls}");
        }
    }
}

#[tokio::test]
async fn principal_lifecycle_requires_exact_inspected_ownership() {
    use std::os::unix::fs::PermissionsExt;
    let owner = RunnerOwner::ApplicationPrincipal {
        application_id: 3,
        workspace_binding_id: 7,
        principal_digest: "a".repeat(64),
    };
    let name = owner.container_name();
    for mode in [
        "valid",
        "missing_manager",
        "foreign_manager",
        "owner_mismatch",
        "name_mismatch",
        "missing_managed",
        "missing_id",
        "inspect_failure",
    ] {
        for reconcile in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let log = temp.path().join("calls");
            let fake = temp.path().join("docker");
            let manager = if mode == "missing_manager" {
                serde_json::Value::Null
            } else if mode == "foreign_manager" {
                serde_json::json!("another-manager")
            } else {
                serde_json::json!("test-manager")
            };
            let key = if mode == "owner_mismatch" {
                format!("application_principal:3:7:{}", "b".repeat(64))
            } else {
                owner.stable_key()
            };
            let inspected_name = if mode == "name_mismatch" {
                "/some-other-container".to_string()
            } else {
                format!("/{name}")
            };
            let metadata = serde_json::json!({"Name": inspected_name, "Id": if mode == "missing_id" { "" } else { "test-id" }, "State":{"Status":"running"}, "Config":{"Labels":{"desk-foreman.manager": manager, "desk-foreman.owner":key, "desk-foreman.managed": if mode == "missing_managed" { "false" } else { "true" }}}, "Mounts":[{"Destination":"/workspace","Source":"/tmp/workspace"}]}).to_string();
            let script = format!(
                "#!/bin/bash\nif [ \"$1\" = \"{READY_ARG}\" ]; then exit 0; fi\necho \"$@\" >> '{}'\ncase \"$1\" in\n ps) echo '{}' ;;\n inspect) {} ;;\nesac\n",
                log.display(),
                name,
                if mode == "inspect_failure" {
                    "echo 'permission denied' >&2; exit 1".to_string()
                } else {
                    format!("printf '%s\\n' '{metadata}'")
                }
            );
            std::fs::write(&fake, script).unwrap();
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
            probe_fixture_readiness(&fake, spawn_fixture_readiness_probe).unwrap();
            let backend = backend_with_docker_cli(fake.to_str().unwrap());
            if reconcile {
                backend.reconcile_containers().await.unwrap();
            } else {
                backend.last_active.lock().unwrap().insert(
                    owner.stable_key(),
                    std::time::Instant::now() - Duration::from_secs(10),
                );
                backend.reclaim_idle_runners_inner().await;
            }
            let calls = std::fs::read_to_string(&log).unwrap();
            if mode == "valid" {
                assert!(calls.lines().any(|line| line == "stop test-id"));
                assert!(calls.lines().any(|line| line == "rm test-id"));
            }
            assert!(
                calls.contains("inspect"),
                "{mode} reconcile={reconcile}: {calls}"
            );
            assert_eq!(
                calls.lines().any(|line| line.starts_with("stop ")),
                mode == "valid",
                "{mode} reconcile={reconcile}: {calls}"
            );
            assert_eq!(
                calls.lines().any(|line| line.starts_with("rm ")),
                mode == "valid",
                "{mode} reconcile={reconcile}: {calls}"
            );
        }
    }
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
