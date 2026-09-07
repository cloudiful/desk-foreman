use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::Context;
use desk_foreman::pathing::resolve_workspace_path;
use runner_protocol::{
    CancelSessionRequest, ExecRequest, InputRequest, RunnerOwner, RunnerSessionStatus,
    RunnerShellRequest, ShellToolOutput,
};
use tokio::sync::Mutex;
use tokio::time::timeout;

use super::{
    RunnerBackend,
    backend::RunnerOperationLease,
    session_gate::{SessionGate, SessionPermit},
    shell_session::ShellSession,
    shell_spawn::{build_command, build_pty_command, open_pty},
};

pub struct ShellManager {
    runner: Arc<dyn RunnerBackend>,
    config: crate::config::SharedRunnerManagerConfig,
    session_gate: Arc<SessionGate>,
    next_session_id: AtomicU64,
    sessions: Mutex<HashMap<u64, Arc<ManagedSession>>>,
}

impl ShellManager {
    pub fn new(
        runner: Arc<dyn RunnerBackend>,
        config: crate::config::SharedRunnerManagerConfig,
    ) -> Self {
        Self {
            runner,
            config,
            session_gate: SessionGate::new(),
            next_session_id: AtomicU64::new(1),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub async fn exec(&self, request: ExecRequest) -> anyhow::Result<ShellToolOutput> {
        self.cleanup_expired_sessions().await;

        let slot = self.session_gate.acquire(&self.config).await;
        let config = self.config.read().await.clone();

        let working_dir = resolve_workspace_path(
            &request.workspace_root,
            request.workdir.as_deref().unwrap_or("."),
        )?;
        let session_id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
        let operation = RunnerOperationLease::new(Arc::clone(&self.runner), request.owner.clone());
        let session = ShellSession::spawn(
            session_id,
            &*self.runner,
            &request,
            working_dir,
            config
                .max_output_bytes
                .min(request.max_output_bytes.unwrap_or(config.max_output_bytes)),
        )
        .await?;

        self.sessions.lock().await.insert(
            session_id,
            Arc::new(ManagedSession {
                owner: request.owner.clone(),
                session_key: request.session_key.clone(),
                session: Arc::clone(&session),
                _operation: operation,
                _slot: slot,
            }),
        );
        let interact = session.interact(
            "",
            request.yield_time_ms,
            request.max_output_tokens,
            request.max_output_bytes,
        );
        let output = if let Some(timeout_ms) = request.timeout_ms {
            match timeout(Duration::from_millis(timeout_ms), interact).await {
                Ok(result) => result,
                Err(_) => {
                    session.kill_timed_out().await?;
                    session
                        .snapshot(request.max_output_tokens, request.max_output_bytes)
                        .await
                }
            }
        } else {
            interact.await
        };
        let output = match output {
            Ok(output) => output,
            Err(error) => {
                self.sessions.lock().await.remove(&session_id);
                let _ = session.kill().await;
                return Err(error);
            }
        };
        if output.session_id.is_none() {
            // Parent exited naturally: still SIGKILL the spawn-captured PGID
            // before dropping the map entry, otherwise backgrounded
            // grandchildren survive with no session to cancel and later
            // write after handover while Direct cleanup (no-op) claims
            // quiescence. Best-effort; takeover re-list plus container stop
            // remains authoritative.
            session.reap_orphan_group().await;
            self.sessions.lock().await.remove(&session_id);
        }
        Ok(output)
    }

    pub async fn write_stdin(&self, request: InputRequest) -> anyhow::Result<ShellToolOutput> {
        self.cleanup_expired_sessions().await;
        let config = self.config.read().await.clone();
        let max_output_bytes = config
            .max_output_bytes
            .min(request.max_output_bytes.unwrap_or(config.max_output_bytes));
        let session = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(&request.session_id)
                .cloned()
                .with_context(|| format!("unknown session_id {}", request.session_id))?
        };
        if session.owner != request.owner || session.session_key != request.session_key {
            anyhow::bail!("session does not belong to current user");
        }
        self.runner.touch_activity(&request.owner);
        let interact = session.session.interact(
            &request.chars,
            request.yield_time_ms,
            request.max_output_tokens,
            Some(max_output_bytes),
        );
        let output = if let Some(timeout_ms) = request.timeout_ms {
            match timeout(Duration::from_millis(timeout_ms), interact).await {
                Ok(result) => result,
                Err(_) => {
                    session.session.kill_timed_out().await?;
                    session
                        .session
                        .snapshot(request.max_output_tokens, Some(max_output_bytes))
                        .await
                }
            }
        } else {
            interact.await
        };
        let output = match output {
            Ok(output) => output,
            Err(error) => {
                self.sessions.lock().await.remove(&request.session_id);
                let _ = session.session.kill().await;
                return Err(error);
            }
        };
        if output.session_id.is_none() {
            // Same orphan-group reap as `exec`: a naturally-exited parent
            // must not leave grandchildren with no map entry to cancel.
            session.session.reap_orphan_group().await;
            self.sessions.lock().await.remove(&request.session_id);
        }
        Ok(output)
    }

    pub async fn cancel_session(
        &self,
        request: CancelSessionRequest,
    ) -> anyhow::Result<RunnerSessionStatus> {
        // Retain-on-failure: look up without removing so a failed
        // termination keeps the entry and later `list_sessions`
        // quiescence checks still observe the live session. Removing
        // before `cancel()` succeeds lets a retry report zero sessions
        // while the old process (or its descendants) still run.
        let managed = {
            let sessions = self.sessions.lock().await;
            let managed = sessions
                .get(&request.session_id)
                .cloned()
                .with_context(|| format!("unknown session_id {}", request.session_id))?;
            if managed.owner != request.owner || managed.session_key != request.session_key {
                anyhow::bail!("session does not belong to current user");
            }
            managed
        };
        // Group-aware kill plus bounded reap; Err retains the map entry.
        // For Docker this reaps the local `docker exec` CLI; the in-container
        // shell is stopped by the takeover helper's `cleanup_runner_owner`
        // container removal (see `docker.rs`). Per-session container removal
        // is intentionally not done here to preserve concurrent sessions.
        managed.session.cancel().await?;
        // Only unlink after successful termination, so success implies
        // map removal plus a dead local group. Docker quiescence additionally
        // requires the helper-level container stop.
        self.sessions.lock().await.remove(&request.session_id);
        managed.status().await
    }

    pub async fn list_sessions(&self) -> anyhow::Result<Vec<RunnerSessionStatus>> {
        self.cleanup_expired_sessions().await;
        let sessions = self.sessions.lock().await;
        let mut result = Vec::with_capacity(sessions.len());
        for managed in sessions.values() {
            result.push(managed.status().await?);
        }
        Ok(result)
    }

    async fn cleanup_expired_sessions(&self) {
        let session_idle_ttl = self.config.read().await.idle_ttl;
        let now = Instant::now();
        let snapshot = {
            let sessions = self.sessions.lock().await;
            sessions
                .iter()
                .map(|(id, session)| (*id, Arc::clone(session)))
                .collect::<Vec<_>>()
        };
        let mut expired_ids = Vec::new();
        for (id, session) in snapshot {
            if now.duration_since(session.session.last_activity().await) > session_idle_ttl {
                expired_ids.push(id);
            }
        }

        // Retain-on-failure like `cancel_session`: only unlink after the
        // group kill succeeds so a failed idle reap stays visible for the
        // next cleanup pass instead of leaking a live process.
        for id in expired_ids {
            let session = {
                let sessions = self.sessions.lock().await;
                sessions.get(&id).cloned()
            };
            let Some(session) = session else {
                continue;
            };
            if session.session.kill().await.is_ok() {
                self.sessions.lock().await.remove(&id);
            }
        }
    }
}

struct ManagedSession {
    owner: RunnerOwner,
    session_key: Option<String>,
    session: Arc<ShellSession>,
    _operation: RunnerOperationLease,
    _slot: SessionPermit,
}

impl ManagedSession {
    async fn status(&self) -> anyhow::Result<RunnerSessionStatus> {
        let mut status = self.session.status().await?;
        status.owner = self.owner.clone();
        status.session_key = self.session_key.clone();
        Ok(status)
    }
}

impl ShellSession {
    async fn spawn(
        session_id: u64,
        runner: &dyn RunnerBackend,
        request: &ExecRequest,
        working_dir: PathBuf,
        max_output_bytes: usize,
    ) -> anyhow::Result<Arc<Self>> {
        let target = runner
            .prepare_shell_spawn(RunnerShellRequest {
                owner: request.owner.clone(),
                workspace_root: request.workspace_root.clone(),
                working_dir,
                shell: request.shell.clone(),
                login: request.login,
                tty: request.tty,
                command: request.cmd.clone(),
                network_enabled: request.network_enabled,
            })
            .await?;
        if request.tty {
            let (pty, pts) = open_pty()?;
            let command = build_pty_command(&target);
            let spawned = command.spawn(pts).context("failed to spawn PTY command")?;
            let (reader, writer_half) = pty.into_split();
            let session = Arc::new(Self::new(
                session_id,
                Box::new(writer_half),
                spawned,
                max_output_bytes,
                true,
            ));
            if let Some(timeout_ms) = request.timeout_ms {
                ShellSession::spawn_timeout_watchdog(
                    Arc::clone(&session),
                    Duration::from_millis(timeout_ms),
                );
            }
            Self::spawn_reader(Arc::clone(&session), reader, false);
            Ok(session)
        } else {
            let mut command = build_command(&target);
            let mut spawned = command.spawn().context("failed to spawn command")?;
            let stdin = spawned.stdin.take().context("stdin not available")?;
            let stdout = spawned.stdout.take().context("stdout not available")?;
            let stderr = spawned.stderr.take().context("stderr not available")?;
            let session = Arc::new(Self::new(
                session_id,
                Box::new(stdin),
                spawned,
                max_output_bytes,
                false,
            ));
            if let Some(timeout_ms) = request.timeout_ms {
                ShellSession::spawn_timeout_watchdog(
                    Arc::clone(&session),
                    Duration::from_millis(timeout_ms),
                );
            }
            Self::spawn_reader(Arc::clone(&session), stdout, false);
            Self::spawn_reader(Arc::clone(&session), stderr, true);
            Ok(session)
        }
    }
}
