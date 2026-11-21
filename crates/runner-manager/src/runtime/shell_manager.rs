use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::Context;
use desk_foreman::pathing::resolve_workspace_path;
use runner_protocol::{
    CancelSessionRequest, ExecRequest, ExecSessionResponse, InputRequest, RetainedSessionState,
    RetainedSessionStatus, RunnerOwner, RunnerSessionStatus, RunnerShellRequest,
    SessionInputRequest, SessionLookupRequest, SessionOutputChunk, SessionOutputRequest,
    SessionStatusResponse, ShellToolOutput,
};
use tokio::sync::Mutex;
use tokio::time::timeout;

use super::{
    RunnerBackend,
    backend::RunnerOperationLease,
    session_gate::{SessionGate, SessionPermit},
    shell_session::{self, ShellSession},
    shell_spawn::{build_command, build_pty_command, open_pty},
};

fn retained_identity_matches(
    retained: bool,
    stored_owner: &RunnerOwner,
    stored_key: &Option<String>,
    stored_execution: &Option<String>,
    owner: &RunnerOwner,
    key: &Option<String>,
    execution: &str,
) -> bool {
    retained
        && stored_owner == owner
        && stored_key == key
        && stored_execution.as_deref() == Some(execution)
}

pub struct ShellManager {
    runner: Arc<dyn RunnerBackend>,
    config: crate::config::SharedRunnerManagerConfig,
    session_gate: Arc<SessionGate>,
    next_session_id: AtomicU64,
    sessions: Mutex<HashMap<u64, Arc<ManagedSession>>>,
    retained_admission: Mutex<()>,
    #[cfg(test)]
    fail_next_cancel: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    expire_before_cancel: std::sync::atomic::AtomicBool,
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
            retained_admission: Mutex::new(()),
            #[cfg(test)]
            fail_next_cancel: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            expire_before_cancel: std::sync::atomic::AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    pub(super) async fn hold_session_map_for_test(&self) -> impl Drop + '_ {
        self.sessions.lock().await
    }

    #[cfg(test)]
    pub(super) fn fail_next_cancel_for_test(&self) {
        self.fail_next_cancel.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(super) async fn expire_before_cancel_for_test(&self) {
        self.config.write().await.idle_ttl = Duration::ZERO;
        self.expire_before_cancel.store(true, Ordering::SeqCst);
    }

    async fn spawn_managed(
        &self,
        request: &ExecRequest,
        execution_key: Option<String>,
    ) -> anyhow::Result<Arc<ManagedSession>> {
        let retain = execution_key.is_some();
        if retain && request.tty {
            anyhow::bail!("retained PTY sessions unsupported");
        }
        if matches!(request.owner, RunnerOwner::ApplicationPrincipal { .. })
            && !self.runner.supports_workspace_principal_stop()
        {
            anyhow::bail!("principal execution is unsupported by this backend");
        }
        self.cleanup_expired_sessions().await;

        let config = self.config.read().await.clone();
        let output_limit = config
            .max_output_bytes
            .min(request.max_output_bytes.unwrap_or(config.max_output_bytes));
        if retain {
            anyhow::ensure!(output_limit > 0, "invalid output cap");
        }
        let working_dir = resolve_workspace_path(
            &request.workspace_root,
            request.workdir.as_deref().unwrap_or("."),
        )?;
        let slot = self.session_gate.acquire(&self.config).await;
        let session_id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
        let operation = RunnerOperationLease::new(Arc::clone(&self.runner), request.owner.clone());
        let target = self
            .runner
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
        let mut sessions = self.sessions.lock().await;
        let session = ShellSession::spawn(session_id, target, request, output_limit, retain)?;
        let managed = Arc::new(ManagedSession {
            owner: request.owner.clone(),
            session_key: request.session_key.clone(),
            execution_key,
            session,
            retain_after_exit: retain,
            lifecycle: Mutex::new(ManagedLifecycle {
                resources: Some((operation, slot)),
                completed_at: None,
            }),
        });
        sessions.insert(session_id, Arc::clone(&managed));
        if retain {
            let timeout_ms = request
                .timeout_ms
                .expect("retained timeout set before spawn");
            Arc::clone(&managed).monitor(
                Duration::from_millis(timeout_ms).saturating_add(Duration::from_secs(300)),
            );
        }
        Ok(managed)
    }

    pub async fn exec(&self, request: ExecRequest) -> anyhow::Result<ShellToolOutput> {
        let managed = self.spawn_managed(&request, None).await?;
        let session = &managed.session;
        let session_id = session.id;
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

    pub async fn exec_session(
        &self,
        input: runner_protocol::ExecSessionRequest,
    ) -> anyhow::Result<ExecSessionResponse> {
        anyhow::ensure!(
            !input.execution_key.is_empty() && input.execution_key.len() <= 128,
            "invalid execution key"
        );
        let mut request = input.request;
        let _admission = self.retained_admission.lock().await;
        self.cleanup_retained(true).await;
        let count = self
            .sessions
            .lock()
            .await
            .values()
            .filter(|s| s.retain_after_exit)
            .count();
        anyhow::ensure!(count < 256, "retained session capacity exhausted");
        let limit = self.config.read().await.max_timeout_ms;
        let timeout_ms = request.timeout_ms.unwrap_or(120_000).min(limit);
        request.timeout_ms = Some(timeout_ms);
        let managed = self
            .spawn_managed(&request, Some(input.execution_key))
            .await?;
        let cap = managed.session.max_output_bytes.min(1024 * 1024);
        let chunk = managed.session.retained_output(None, cap).await?;
        Ok(ExecSessionResponse {
            session_id: managed.session.id,
            chunk,
        })
    }

    async fn retained_lookup(
        &self,
        owner: &RunnerOwner,
        key: &Option<String>,
        execution: &str,
        id: u64,
    ) -> anyhow::Result<Arc<ManagedSession>> {
        self.cleanup_retained(false).await;
        let managed = self
            .sessions
            .lock()
            .await
            .get(&id)
            .cloned()
            .with_context(|| format!("unknown session_id {id}"))?;
        anyhow::ensure!(
            retained_identity_matches(
                managed.retain_after_exit,
                &managed.owner,
                &managed.session_key,
                &managed.execution_key,
                owner,
                key,
                execution
            ),
            "unknown session_id {id}"
        );
        Ok(managed)
    }

    pub async fn session_status(
        &self,
        request: SessionLookupRequest,
    ) -> anyhow::Result<SessionStatusResponse> {
        let managed = self
            .retained_lookup(
                &request.owner,
                &request.session_key,
                &request.execution_key,
                request.session_id,
            )
            .await?;
        managed.observe_terminal().await?;
        Ok(SessionStatusResponse {
            status: Some(managed.session.retained_status().await?),
        })
    }

    pub async fn session_output(
        &self,
        request: SessionOutputRequest,
    ) -> anyhow::Result<SessionOutputChunk> {
        let managed = self
            .retained_lookup(
                &request.owner,
                &request.session_key,
                &request.execution_key,
                request.session_id,
            )
            .await?;
        managed.observe_terminal().await?;
        let cap = request
            .max_bytes
            .min(self.config.read().await.max_output_bytes)
            .min(managed.session.max_output_bytes)
            .min(1024 * 1024);
        managed
            .session
            .retained_output(request.cursor.as_deref(), cap)
            .await
    }

    pub async fn session_input(&self, request: SessionInputRequest) -> anyhow::Result<()> {
        let managed = self
            .retained_lookup(
                &request.owner,
                &request.session_key,
                &request.execution_key,
                request.session_id,
            )
            .await?;
        self.runner.touch_activity(&request.owner);
        managed.session.retained_input(&request.chars).await
    }

    pub async fn session_cancel(
        &self,
        request: SessionLookupRequest,
    ) -> anyhow::Result<RetainedSessionStatus> {
        let managed = self
            .retained_lookup(
                &request.owner,
                &request.session_key,
                &request.execution_key,
                request.session_id,
            )
            .await?;
        if managed.observe_terminal().await?.state == RetainedSessionState::Running {
            managed.session.cancel().await?;
        }
        managed.observe_terminal().await?;
        managed.session.retained_status().await
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
        if session.retain_after_exit
            || session.owner != request.owner
            || session.session_key != request.session_key
        {
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
                // An input failure keeps the session for later poll, cancel or TTL.
                if !shell_session::is_input_failure(&error) {
                    self.sessions.lock().await.remove(&request.session_id);
                    let _ = session.session.kill().await;
                }
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
        #[cfg(test)]
        if self.expire_before_cancel.swap(false, Ordering::SeqCst) {
            self.cleanup_expired_sessions().await;
        }
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
        if managed.retain_after_exit {
            if managed.observe_terminal().await?.state == RetainedSessionState::Running {
                managed.session.cancel().await?;
            }
            managed.observe_terminal().await?;
            return managed.status().await;
        }
        #[cfg(test)]
        if self.fail_next_cancel.swap(false, Ordering::SeqCst) {
            anyhow::bail!("injected cancellation failure");
        }
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
        self.snapshot_sessions().await
    }

    pub(super) async fn snapshot_sessions(&self) -> anyhow::Result<Vec<RunnerSessionStatus>> {
        let sessions: Vec<_> = self.sessions.lock().await.values().cloned().collect();
        let mut result = Vec::with_capacity(sessions.len());
        for managed in sessions {
            if managed.retain_after_exit
                && managed.observe_terminal().await?.state != RetainedSessionState::Running
            {
                continue;
            }
            result.push(managed.status().await?);
        }
        Ok(result)
    }

    async fn cleanup_retained(&self, reserve: bool) {
        let snapshot: Vec<_> = self
            .sessions
            .lock()
            .await
            .iter()
            .filter(|(_, s)| s.retain_after_exit)
            .map(|(id, s)| (*id, Arc::clone(s)))
            .collect();
        let mut terminal = Vec::new();
        for (id, session) in &snapshot {
            if let Some(at) = session.lifecycle.lock().await.completed_at {
                terminal.push((*id, at));
            }
        }
        let remove = retained_evictions(snapshot.len(), terminal, Instant::now(), reserve);
        let mut sessions = self.sessions.lock().await;
        for id in remove {
            sessions.remove(&id);
        }
    }

    async fn cleanup_expired_sessions(&self) {
        self.cleanup_retained(false).await;
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
            if session.retain_after_exit {
                continue;
            }
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

fn retained_evictions(
    total: usize,
    mut terminal: Vec<(u64, Instant)>,
    now: Instant,
    reserve: bool,
) -> Vec<u64> {
    terminal.sort_by_key(|(_, at)| *at);
    let mut remaining = total;
    let mut remove = Vec::new();
    for (id, at) in terminal {
        if now.saturating_duration_since(at) >= Duration::from_secs(300)
            || remaining > if reserve { 255 } else { 256 }
        {
            remove.push(id);
            remaining -= 1;
        }
    }
    remove
}

#[cfg(test)]
mod retained_lifecycle_tests {
    #[test]
    fn retained_lifecycle_restart_identity_does_not_alias() {
        let principal = Some("principal".to_owned());
        let execution = Some("execution-b".to_owned());
        let owner = runner_protocol::RunnerOwner::InternalUser { user_id: 1 };
        let other = runner_protocol::RunnerOwner::InternalUser { user_id: 2 };
        assert!(super::retained_identity_matches(
            true,
            &owner,
            &principal,
            &execution,
            &owner,
            &principal,
            "execution-b"
        ));
        assert!(!super::retained_identity_matches(
            true,
            &owner,
            &principal,
            &execution,
            &owner,
            &principal,
            "execution-a"
        ));
        assert!(!super::retained_identity_matches(
            true,
            &owner,
            &principal,
            &execution,
            &other,
            &principal,
            "execution-b"
        ));
        assert!(!super::retained_identity_matches(
            false,
            &owner,
            &principal,
            &execution,
            &owner,
            &principal,
            "execution-b"
        ));
    }
    use super::*;

    #[test]
    fn retained_lifecycle_read_expiry_boundary_protects_active() {
        let now = Instant::now();
        let before = now - Duration::from_secs(300) + Duration::from_nanos(1);
        let boundary = now - Duration::from_secs(300);
        assert_eq!(
            retained_evictions(3, vec![(1, before), (2, boundary)], now, false),
            vec![2]
        );
        assert!(retained_evictions(1, vec![], now, false).is_empty());
    }

    #[test]
    fn retained_lifecycle_eviction_protects_active_and_orders_terminal() {
        let now = Instant::now();
        let old = now - Duration::from_secs(301);
        let recent = now - Duration::from_secs(1);
        assert_eq!(
            retained_evictions(256, vec![(1, recent), (2, old)], now, true),
            vec![2]
        );
        assert_eq!(
            retained_evictions(256, vec![], now, true),
            Vec::<u64>::new()
        );
        assert_eq!(
            retained_evictions(256, vec![(1, recent)], now, false),
            Vec::<u64>::new()
        );
        assert_eq!(
            retained_evictions(256, vec![(1, recent)], now, true),
            vec![1]
        );
    }
}

struct ManagedSession {
    owner: RunnerOwner,
    session_key: Option<String>,
    execution_key: Option<String>,
    session: Arc<ShellSession>,
    retain_after_exit: bool,
    lifecycle: Mutex<ManagedLifecycle>,
}

struct ManagedLifecycle {
    resources: Option<(RunnerOperationLease, SessionPermit)>,
    completed_at: Option<Instant>,
}

impl ManagedSession {
    async fn observe_terminal(&self) -> anyhow::Result<RetainedSessionStatus> {
        let status = self.session.observe_retained().await?;
        if status.state != RetainedSessionState::Running {
            let mut lifecycle = self.lifecycle.lock().await;
            if lifecycle.completed_at.is_none() {
                super::process_termination::kill_orphan_group(self.session.pgid).await?;
                lifecycle.completed_at = Some(Instant::now());
                lifecycle.resources.take();
            }
        }
        Ok(status)
    }

    fn monitor(self: Arc<Self>, duration: Duration) {
        tokio::spawn(async move {
            let started = Instant::now();
            loop {
                match self.observe_terminal().await {
                    Ok(status) if status.state != RetainedSessionState::Running => break,
                    Err(error) => {
                        tracing::warn!(session_id = self.session.id, %error, "retained monitor observation failed")
                    }
                    _ => {}
                }
                if started.elapsed() >= duration {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
    }

    async fn status(&self) -> anyhow::Result<RunnerSessionStatus> {
        let mut status = self.session.status().await?;
        if self.retain_after_exit {
            let retained = self.session.retained_status().await?;
            status.state = match retained.state {
                RetainedSessionState::Running => "running",
                RetainedSessionState::Exited => "exited",
                RetainedSessionState::Cancelled => "cancelled",
                RetainedSessionState::TimedOut => "timed_out",
            }
            .to_string();
            status.exit_code = retained.exit_code;
            status.timed_out = retained.state == RetainedSessionState::TimedOut;
        }
        status.owner = self.owner.clone();
        status.session_key = self.session_key.clone();
        Ok(status)
    }
}

impl ShellSession {
    fn spawn(
        session_id: u64,
        target: super::ProcessSpawnTarget,
        request: &ExecRequest,
        max_output_bytes: usize,
        retain_output: bool,
    ) -> anyhow::Result<Arc<Self>> {
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
                retain_output,
            ));
            if let Some(timeout_ms) = request.timeout_ms {
                ShellSession::spawn_timeout_watchdog(
                    Arc::clone(&session),
                    Duration::from_millis(timeout_ms),
                    retain_output,
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
                retain_output,
            ));
            if let Some(timeout_ms) = request.timeout_ms {
                ShellSession::spawn_timeout_watchdog(
                    Arc::clone(&session),
                    Duration::from_millis(timeout_ms),
                    retain_output,
                );
            }
            Self::spawn_reader(Arc::clone(&session), stdout, false);
            Self::spawn_reader(Arc::clone(&session), stderr, true);
            Ok(session)
        }
    }
}
