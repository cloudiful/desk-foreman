//! Takeover termination regression: quiescence must mean dead groups.
//!
//! These are real-subprocess tests (not classifiers). Each spawns a
//! test-owned shell whose background descendant would survive a
//! parent-only SIGKILL. Cancelling must kill the whole process group so a
//! retry's `list_sessions` quiescence check cannot report zero while old
//! execution still writes.
//!
//! Only short-lived test-owned subprocesses are used: background `sleep`
//! writers are SIGKILLed within milliseconds of `cancel_session`; no user
//! services or unrelated processes are touched. Docker `exec` descendants
//! are explicitly out of scope here (see `process_termination` docs) and
//! must be covered once the docker termination paths are allowlisted.

use std::{sync::Arc, time::Duration};

use desk_foreman::runner::RunnerService;
use runner_protocol::{CancelSessionRequest, ExecRequest, RunnerOwner};
use tempfile::TempDir;

use super::{DirectRunnerBackend, LocalRunnerService};

fn test_config(workspace_root: &std::path::Path) -> crate::config::RunnerManagerConfig {
    crate::config::RunnerManagerConfig {
        control_plane_url: None,
        manager_id: "termination-test".to_string(),
        bind_addr: "127.0.0.1:0".to_string(),
        auth_token: "test-token".to_string(),
        backend: crate::config::RunnerBackendKind::Direct,
        workspace_root: workspace_root.to_path_buf(),
        host_workspace_root: workspace_root.to_path_buf(),
        image: "test-image".to_string(),
        workdir: "/workspace".to_string(),
        network_enabled: false,
        max_output_bytes: 262_144,
        max_timeout_ms: 600_000,
        max_sessions: 8,
        pids_limit: 256,
        memory_limit: "1g".to_string(),
        cpu_limit: "2".to_string(),
        idle_ttl: Duration::from_secs(60),
        docker_cli: "docker".to_string(),
        docker_host: None,
        runtime_class: None,
    }
}

async fn test_service(workspace_root: &std::path::Path) -> Arc<LocalRunnerService> {
    let config = Arc::new(tokio::sync::RwLock::new(test_config(workspace_root)));
    LocalRunnerService::new(DirectRunnerBackend::new(), config)
}

fn binding_owner() -> RunnerOwner {
    RunnerOwner::WorkspaceBinding {
        workspace_binding_id: 42,
    }
}

fn exec_request(workspace_root: &std::path::Path, cmd: String) -> ExecRequest {
    ExecRequest {
        owner: binding_owner(),
        session_key: None,
        workspace_root: workspace_root.to_path_buf(),
        cmd,
        workdir: None,
        shell: "bash".to_string(),
        login: false,
        tty: false,
        timeout_ms: None,
        yield_time_ms: Some(50),
        max_output_tokens: None,
        max_output_bytes: None,
        network_enabled: false,
    }
}

/// Background writer must not survive cancellation.
///
/// Old parent-only kill leaves `(sleep 0.6; echo SURVIVED >> marker) &`
/// alive after the bash dies, so the marker appears even though
/// `list_sessions` is empty (false quiescence). Group kill must prevent
/// the write.
#[tokio::test]
async fn cancel_kills_descendant_writer_before_it_writes() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let workspace_root = temp.path().canonicalize()?;
    let marker = workspace_root.join("marker.txt");
    std::fs::write(&marker, "")?;
    let marker_str = marker.to_string_lossy().to_string();

    let service = test_service(&workspace_root).await;
    // Background writer fires after 0.6s; `wait` keeps the parent alive so
    // `exec` returns a live session we can cancel immediately.
    let cmd = format!("(sleep 0.6; echo SURVIVED >> \"{marker_str}\") & echo started; wait");
    let output = service
        .exec_shell(exec_request(&workspace_root, cmd))
        .await?;
    let session_id = output.session_id.expect("expected live session for wait");
    assert!(output.output.contains("started"));

    service
        .cancel_session(CancelSessionRequest {
            owner: binding_owner(),
            session_key: None,
            session_id,
        })
        .await?;

    // Quiescence: no sessions remain for the owner.
    let remaining = service.list_sessions().await?;
    assert!(
        remaining.is_empty(),
        "cancel must leave zero sessions, got {remaining:?}"
    );

    // Give the background writer past its 0.6s deadline; it must not run.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let contents = std::fs::read_to_string(&marker)?;
    assert!(
        !contents.contains("SURVIVED"),
        "descendant writer survived parent-only kill; group termination failed"
    );
    Ok(())
}

/// Background `sleep` group must be reaped, not orphaned.
///
/// Captures the grandchild PID via `$!`, cancels, then asserts the PID is
/// gone (`/proc` absent) and the session map is empty. A parent-only kill
/// would leave `sleep` reparented and visible.
#[tokio::test]
async fn cancel_kills_background_sleep_process_group() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let workspace_root = temp.path().canonicalize()?;
    let pidfile = workspace_root.join("child.pid");
    let pidfile_str = pidfile.to_string_lossy().to_string();

    let service = test_service(&workspace_root).await;
    let cmd = format!("sleep 10 & echo $! > \"{pidfile_str}\"; wait");
    let output = service
        .exec_shell(exec_request(&workspace_root, cmd))
        .await?;
    let session_id = output.session_id.expect("expected live session");

    // Wait for the background pid to be recorded (bounded, test-owned).
    let child_pid: u32 = {
        let mut pid = None;
        for _ in 0..50 {
            if let Ok(raw) = std::fs::read_to_string(&pidfile) {
                if let Ok(parsed) = raw.trim().parse::<u32>() {
                    pid = Some(parsed);
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        pid.expect("background sleep pid should be recorded")
    };

    service
        .cancel_session(CancelSessionRequest {
            owner: binding_owner(),
            session_key: None,
            session_id,
        })
        .await?;

    let remaining = service.list_sessions().await?;
    assert!(
        remaining.is_empty(),
        "expected quiescence, got {remaining:?}"
    );

    // Retry must report unknown (entry removed only on success), not a
    // second successful cancel.
    let retry = service
        .cancel_session(CancelSessionRequest {
            owner: binding_owner(),
            session_key: None,
            session_id,
        })
        .await;
    assert!(
        retry.is_err(),
        "second cancel must fail with unknown session"
    );

    // The grandchild must be gone. Poll briefly: SIGKILL is async and
    // init must reap the orphan on old kernels.
    let proc_path = format!("/proc/{child_pid}");
    let mut gone = false;
    for _ in 0..50 {
        if !std::path::Path::new(&proc_path).exists() {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(gone, "background sleep {child_pid} survived group kill");
    Ok(())
}

/// Failed cancellation must retain the session for retry.
///
/// An unknown session id must not clear unrelated live sessions: the
/// quiescence check must still observe them.
#[tokio::test]
async fn failed_cancel_retains_unrelated_live_session() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let workspace_root = temp.path().canonicalize()?;
    let service = test_service(&workspace_root).await;

    let output = service
        .exec_shell(exec_request(&workspace_root, "cat".to_string()))
        .await?;
    let session_id = output.session_id.expect("expected live cat session");

    // Unknown id fails without touching the live session.
    let bad = service
        .cancel_session(CancelSessionRequest {
            owner: binding_owner(),
            session_key: None,
            session_id: session_id + 9999,
        })
        .await;
    assert!(bad.is_err());

    let remaining = service.list_sessions().await?;
    assert_eq!(remaining.len(), 1, "failed cancel must retain live session");
    assert_eq!(remaining[0].session_id, session_id);

    // Cleanup: real cancel must succeed and reach quiescence.
    service
        .cancel_session(CancelSessionRequest {
            owner: binding_owner(),
            session_key: None,
            session_id,
        })
        .await?;
    assert!(service.list_sessions().await?.is_empty());
    Ok(())
}

/// Orphan grandchildren must not survive a naturally-exited parent.
///
/// Regression for review 4152 P1: `(sleep 0.6; echo SURVIVED >> marker) &
/// exit 0` makes the parent bash exit immediately, so `try_wait` is `Some`
/// and the old fast path returned `Ok` without ever signalling the PGID.
/// The map entry is auto-removed on completion, `list_sessions` is empty,
/// and Direct cleanup (no-op) would claim quiescence while the reparented
/// writer still runs. The fix retains the spawn-captured PGID and always
/// group-kills, including on the exec success-with-exit path.
#[tokio::test]
async fn natural_exit_still_kills_orphan_writer_group() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let workspace_root = temp.path().canonicalize()?;
    let marker = workspace_root.join("orphan-marker.txt");
    std::fs::write(&marker, "")?;
    let marker_str = marker.to_string_lossy().to_string();

    let service = test_service(&workspace_root).await;
    // Parent exits at once; the background writer fires after 0.6s with no
    // session left to cancel. Real stop requires the exec completion path
    // to reap the stored PGID, not just drop the map entry.
    let cmd = format!("(sleep 0.6; echo SURVIVED >> \"{marker_str}\") & exit 0");
    let output = service
        .exec_shell(exec_request(&workspace_root, cmd))
        .await?;
    assert!(
        output.session_id.is_none(),
        "parent should have exited, leaving no live session, got {output:?}"
    );
    assert!(
        service.list_sessions().await?.is_empty(),
        "completed session must be auto-removed"
    );

    // Past the writer deadline: a surviving orphan would have written.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let contents = std::fs::read_to_string(&marker)?;
    assert!(
        !contents.contains("SURVIVED"),
        "orphan grandchild survived parent exit; stored-PGID group kill failed"
    );
    Ok(())
}

#[allow(dead_code)]
fn _keep_tempdir_alive(_dir: &TempDir) {}
