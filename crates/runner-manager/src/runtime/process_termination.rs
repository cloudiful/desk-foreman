//! Process-group termination for shell sessions.
//!
//! Takeover quiescence requires that cancelling a session actually stops
//! old-owner writes, not just that the runner map is empty. Killing only
//! the direct `bash` child leaves its grandchildren alive (reparented to
//! init), so a `sleep & wait` or background writer keeps running and can
//! still write the workspace after the lease transferred.
//!
//! Containment: non-PTY shells are spawned in a fresh process group via
//! `Command::process_group(0)` (see `shell_spawn::build_command`), so the
//! child's PID equals its PGID. PTY shells are already session leaders via
//! `pty-process` (`setsid` in `spawn_impl`), so the same PID==PGID invariant
//! holds. Termination therefore signals the whole group with
//! `/bin/kill -KILL -- -<pgid>` (the host `kill` binary, not the `dash`
//! builtin which rejects negative PIDs) and then reaps the direct child.
//!
//! Tokio semantics (verified against tokio 1.53.1
//! `src/process/mod.rs`): `Child::kill().await` is `start_kill()` (SIGKILL
//! to the direct child only) followed by `wait()` (reap). `start_kill()`
//! alone sends SIGKILL without reaping (the std `Child::kill` equivalent).
//! Neither touches grandchildren, hence the explicit group kill first.
//! We use `start_kill()` plus a bounded `wait()` so a stuck child cannot
//! block takeover indefinitely; `kill().await` would wait unbounded.
//!
//! Docker coverage: for the docker backend the local child is the
//! `docker exec -i` CLI. Killing its process group stops the CLI but does
//! NOT stop the exec'd shell inside the container (server-side), so the
//! takeover helper additionally stops the task-owned container via
//! `cleanup_runner_owner` (`docker stop`+`rm` in `docker.rs`). That call
//! preserves workspace files (host bind mount) and recreates the container
//! on demand. This module covers local group termination for both
//! backends; Docker quiescence requires both layers.

use std::{process::Stdio, time::Duration};

use anyhow::Context;
use tokio::{process::Child, time::timeout};

/// Bounded wait after SIGKILL for the direct child to be reaped.
pub const SESSION_TERMINATE_WAIT: Duration = Duration::from_secs(5);

/// Timeout for the helper `kill` invocation itself.
const GROUP_KILL_TIMEOUT: Duration = Duration::from_secs(5);

/// Put a non-PTY command in its own process group and arm kill-on-drop.
///
/// `process_group(0)` makes the child's PID its PGID (`setpgid(0, 0)` in
/// the child before exec). Callers can then use the child's `id()` as the
/// group id for `kill_process_group`. `kill_on_drop(true)` is a second
/// layer: if the `Child` handle is dropped without an explicit wait, the
/// runtime sends SIGKILL to the direct child (still not the group; the
/// group kill in `terminate_child` remains required).
pub fn configure_process_group(cmd: &mut tokio::process::Command) {
    #[cfg(unix)]
    {
        cmd.process_group(0);
    }
    cmd.kill_on_drop(true);
}

/// Signal the whole process group with SIGKILL.
///
/// Best-effort: an already-dead group (`kill` exit 1 / ESRCH) is success.
/// Uses the host `/bin/kill` binary because `sh` on this host is `dash`
/// whose builtin rejects negative PIDs (`Illegal number`). Falls back to
/// `/usr/bin/kill`, `kill` in PATH, then `bash -c "kill ..."` (bash
/// builtin supports group syntax).
pub async fn kill_process_group(pgid: u32) -> anyhow::Result<()> {
    let target = format!("-{pgid}");
    let candidates: Vec<(String, Vec<String>)> = vec![
        (
            "/bin/kill".to_string(),
            vec!["-KILL".to_string(), "--".to_string(), target.clone()],
        ),
        (
            "/usr/bin/kill".to_string(),
            vec!["-KILL".to_string(), "--".to_string(), target.clone()],
        ),
        (
            "kill".to_string(),
            vec!["-KILL".to_string(), "--".to_string(), target.clone()],
        ),
    ];
    let mut last_error: Option<anyhow::Error> = None;
    for (program, args) in candidates {
        match run_kill_binary(&program, &args).await {
            Ok(()) => return Ok(()),
            Err(error) => {
                let message = format!("{error:#}");
                // Binary missing: try next candidate. Anything else (e.g.
                // group already gone, treated as Ok inside) propagates.
                if message.contains("No such file")
                    || message.contains("not found")
                    || message.contains("failed to spawn kill")
                {
                    last_error = Some(error);
                    continue;
                }
                return Err(error);
            }
        }
    }
    // Last resort: bash builtin supports `kill -KILL -- -<pgid>`.
    match run_bash_kill(&target).await {
        Ok(()) => Ok(()),
        Err(error) => Err(last_error.unwrap_or(error)),
    }
}

async fn run_kill_binary(program: &str, args: &[String]) -> anyhow::Result<()> {
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = timeout(GROUP_KILL_TIMEOUT, cmd.status())
        .await
        .context("timed out invoking kill for process group")?
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!("failed to spawn kill {program}: {error}")
            } else {
                anyhow::anyhow!("failed to spawn kill {program}: {error}")
            }
        })?;
    // Exit 0: signalled. Exit 1 from procps kill is ESRCH (no such
    // process / group already gone): already quiescent, treat as success.
    if status.success() || status.code() == Some(1) {
        Ok(())
    } else {
        anyhow::bail!("kill {program} for process group exited with {status}");
    }
}

async fn run_bash_kill(target: &str) -> anyhow::Result<()> {
    let script = format!("kill -KILL -- {target}");
    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("-c")
        .arg(&script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = timeout(GROUP_KILL_TIMEOUT, cmd.status())
        .await
        .context("timed out invoking bash kill for process group")?
        .context("failed to spawn bash kill")?;
    if status.success() || status.code() == Some(1) {
        Ok(())
    } else {
        anyhow::bail!("bash kill for process group exited with {status}");
    }
}

/// Terminate a session child and its process group, then reap it.
///
/// The caller must pass the PGID captured at spawn (`ShellSession::pgid`).
/// `child.id()` alone is insufficient: after `try_wait` reaps an exited
/// parent it becomes `None`, losing the orphan group identity (review 4152
/// P1). Steps: poll `try_wait` for the direct-child state, always SIGKILL
/// the stored group when known (even when the parent already exited, since
/// grandchildren survive reparented with the same PGID), then skip the
/// direct SIGKILL+wait fast path only when the parent was already reaped.
/// Returns Err on spawn failure or when the bounded wait times out, so
/// callers retain the session for retry instead of reporting false
/// quiescence.
pub async fn terminate_child(child: &mut Child) -> anyhow::Result<()> {
    let pgid = child.id();
    terminate_child_with_timeout(child, pgid, SESSION_TERMINATE_WAIT).await
}

pub async fn terminate_child_with_timeout(
    child: &mut Child,
    stored_pgid: Option<u32>,
    wait_timeout: Duration,
) -> anyhow::Result<()> {
    let exited = child
        .try_wait()
        .context("failed to poll child status")?
        .is_some();
    // Group kill always when the group identity is known, even when the
    // parent already exited: orphans keep the PGID after reparenting.
    // Best-effort: log-and-continue; the direct kill below remains
    // authoritative for the child's own liveness when still running.
    let pgid = stored_pgid.or_else(|| child.id());
    if let Some(pgid) = pgid {
        if let Err(error) = kill_process_group(pgid).await {
            tracing::warn!(pgid, error = %error, "process-group kill failed; falling back to direct kill");
        }
    }
    if exited {
        return Ok(());
    }
    // `start_kill` is SIGKILL to the direct child only (no reap). ESRCH
    // here means the group kill already reaped-or-killed it; re-check.
    match child.start_kill() {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Already gone between try_wait and kill.
        }
        Err(error) => return Err(error).context("failed to SIGKILL session child"),
    }
    match timeout(wait_timeout, child.wait()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) => Err(error).context("failed waiting for killed session child"),
        Err(_) => {
            anyhow::bail!("timed out after {wait_timeout:?} waiting for killed session child")
        }
    }
}

/// Best-effort group kill for a naturally-exited parent.
///
/// Used on the exec/write_stdin success-with-exit path before the map entry
/// is removed: the parent already reaped, but orphans in the stored PGID
/// may still run. Failures are returned so the caller can retain instead
/// of claiming quiescence, but most callers log-and-continue since the map
/// entry is going away either way; the takeover re-list plus container
/// stop remains the authoritative gate.
pub async fn kill_orphan_group(stored_pgid: Option<u32>) -> anyhow::Result<()> {
    let Some(pgid) = stored_pgid else {
        return Ok(());
    };
    kill_process_group(pgid).await
}
