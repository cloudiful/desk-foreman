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
use tokio::sync::{Mutex, Semaphore};
use tokio::time::timeout;

use super::{
    RunnerBackend,
    shell_session::ShellSession,
    shell_spawn::{build_command, build_pty_command, open_pty},
};

pub struct ShellManager {
    runner: Arc<dyn RunnerBackend>,
    session_idle_ttl: Duration,
    max_output_bytes: usize,
    session_slots: Arc<Semaphore>,
    next_session_id: AtomicU64,
    sessions: Mutex<HashMap<u64, Arc<ManagedSession>>>,
}

impl ShellManager {
    pub fn new(
        runner: Arc<dyn RunnerBackend>,
        session_idle_ttl: Duration,
        max_output_bytes: usize,
        max_sessions: usize,
    ) -> Self {
        Self {
            runner,
            session_idle_ttl,
            max_output_bytes,
            session_slots: Arc::new(Semaphore::new(max_sessions.max(1))),
            next_session_id: AtomicU64::new(1),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub async fn exec(&self, request: ExecRequest) -> anyhow::Result<ShellToolOutput> {
        self.cleanup_expired_sessions().await;

        let slot = self
            .session_slots
            .clone()
            .try_acquire_owned()
            .context("shell session limit reached")?;

        let working_dir = resolve_workspace_path(
            &request.workspace_root,
            request.workdir.as_deref().unwrap_or("."),
        )?;
        let session_id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
        let session = ShellSession::spawn(
            session_id,
            &*self.runner,
            &request,
            working_dir,
            self.max_output_bytes
                .min(request.max_output_bytes.unwrap_or(self.max_output_bytes)),
        )
        .await?;

        self.sessions.lock().await.insert(
            session_id,
            Arc::new(ManagedSession {
                owner: request.owner.clone(),
                session_key: request.session_key.clone(),
                session: Arc::clone(&session),
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
        }?;
        if output.session_id.is_none() {
            self.sessions.lock().await.remove(&session_id);
        }
        Ok(output)
    }

    pub async fn write_stdin(&self, request: InputRequest) -> anyhow::Result<ShellToolOutput> {
        self.cleanup_expired_sessions().await;
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
        let interact = session.session.interact(
            &request.chars,
            request.yield_time_ms,
            request.max_output_tokens,
            request.max_output_bytes,
        );
        let output = if let Some(timeout_ms) = request.timeout_ms {
            match timeout(Duration::from_millis(timeout_ms), interact).await {
                Ok(result) => result,
                Err(_) => {
                    session.session.kill_timed_out().await?;
                    session
                        .session
                        .snapshot(request.max_output_tokens, request.max_output_bytes)
                        .await
                }
            }
        } else {
            interact.await
        }?;
        if output.session_id.is_none() {
            self.sessions.lock().await.remove(&request.session_id);
        }
        Ok(output)
    }

    pub async fn cancel_session(
        &self,
        request: CancelSessionRequest,
    ) -> anyhow::Result<RunnerSessionStatus> {
        let managed = {
            let mut sessions = self.sessions.lock().await;
            let managed = sessions
                .get(&request.session_id)
                .cloned()
                .with_context(|| format!("unknown session_id {}", request.session_id))?;
            if managed.owner != request.owner || managed.session_key != request.session_key {
                anyhow::bail!("session does not belong to current user");
            }
            sessions.remove(&request.session_id);
            managed
        };
        managed.session.cancel().await?;
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
            if now.duration_since(session.session.last_activity().await) > self.session_idle_ttl {
                expired_ids.push(id);
            }
        }

        let mut sessions = self.sessions.lock().await;
        for id in expired_ids {
            if let Some(session) = sessions.remove(&id) {
                let _ = session.session.kill().await;
            }
        }
        let active_owners = sessions
            .values()
            .map(|session| session.owner.clone())
            .collect::<Vec<_>>();
        drop(sessions);
        let _ = self.runner.reclaim_idle_runners(active_owners).await;
    }
}

struct ManagedSession {
    owner: RunnerOwner,
    session_key: Option<String>,
    session: Arc<ShellSession>,
    _slot: tokio::sync::OwnedSemaphorePermit,
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
