//! Lease-related runner session helpers.
//!
//! Houses the best-effort session cancellation helper used by the lease
//! takeover endpoint. Keeping the helper (and its focused test fixture)
//! separate from the admin workspace binding router keeps the router file
//! under the repository's 400-line hard cap and gives the cancellation
//! logic a single, well-scoped home.

use runner_protocol::{CancelSessionRequest, RunnerOwner};

use crate::{AppState, db::types::WorkspaceLeaseCancellationOutcome};

/// Best-effort cancel of every runner session scoped to the binding.
///
/// Cancels every session whose `RunnerOwner::WorkspaceBinding { workspace_binding_id }`
/// matches `binding_id`. The helper is binding-scoped, not strictly
/// previous-owner scoped: any runner session that targets the binding is
/// cancelled because such sessions share filesystem and runner state with
/// the displaced lease.
///
/// Errors are surfaced via the returned [`WorkspaceLeaseCancellationOutcome`]
/// rather than propagating as `AppError`, so the takeover response stays
/// deterministic and the cancellation outcome is reported alongside the lease
/// transfer (and recorded in the audit log) without rolling back the
/// already-committed lease change.
pub async fn cancel_binding_sessions_best_effort(
    state: &AppState,
    binding_id: i64,
) -> WorkspaceLeaseCancellationOutcome {
    let mut outcome = WorkspaceLeaseCancellationOutcome {
        attempted: true,
        // Default to success; failure paths explicitly flip this to false.
        succeeded: true,
        sessions_cancelled: 0,
        error: None,
    };
    let sessions = match state.runner.list_sessions().await {
        Ok(sessions) => sessions,
        Err(error) => {
            outcome.succeeded = false;
            outcome.error = Some(format!("failed to list runner sessions: {error}"));
            tracing::warn!(
                workspace_binding_id = binding_id,
                error = %error,
                "failed to list runner sessions during lease takeover"
            );
            return outcome;
        }
    };
    for session in sessions {
        if session.owner
            != (RunnerOwner::WorkspaceBinding {
                workspace_binding_id: binding_id,
            })
        {
            continue;
        }
        match state
            .runner
            .cancel_session(CancelSessionRequest {
                owner: session.owner.clone(),
                session_key: session.session_key,
                session_id: session.session_id,
            })
            .await
        {
            Ok(_) => outcome.sessions_cancelled += 1,
            Err(error) => {
                outcome.succeeded = false;
                let message = format!(
                    "failed to cancel session {} for binding {binding_id}: {error}",
                    session.session_id
                );
                tracing::warn!(
                    workspace_binding_id = binding_id,
                    session_id = session.session_id,
                    error = %error,
                    "failed to cancel runner session during lease takeover"
                );
                outcome
                    .error
                    .get_or_insert_with(|| message.clone())
                    .clone_from(&message);
            }
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::Arc,
        time::{Duration, SystemTime},
    };

    use runner_protocol::{
        CancelSessionRequest, CommandOutput, ExecRequest, InputRequest, RunnerCommandRequest,
        RunnerOwner, RunnerSessionStatus, ShellToolOutput,
    };
    use sqlx::postgres::PgPoolOptions;
    use tokio::sync::Mutex;

    use super::cancel_binding_sessions_best_effort;
    use crate::{
        AppState,
        config::AppConfig,
        policy::{ALL_SCOPES, ResourceLimits},
        runner::{RunnerBroker, RunnerFuture, RunnerService},
    };

    /// Records every `list_sessions` / `cancel_session` call so tests can
    /// assert that the takeover helper only touches sessions owned by the
    /// target binding and tolerates per-session failures without aborting.
    #[derive(Default)]
    pub(super) struct FakeRunnerService {
        sessions: Mutex<Vec<RunnerSessionStatus>>,
    }

    impl FakeRunnerService {
        fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }
    }

    impl RunnerService for FakeRunnerService {
        fn exec_shell<'a>(
            &'a self,
            _request: ExecRequest,
        ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
            Box::pin(async move { anyhow::bail!("exec_shell not supported in fake") })
        }

        fn write_stdin<'a>(
            &'a self,
            _request: InputRequest,
        ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
            Box::pin(async move { anyhow::bail!("write_stdin not supported in fake") })
        }

        fn cancel_session<'a>(
            &'a self,
            request: CancelSessionRequest,
        ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>> {
            Box::pin(async move {
                let mut sessions = self.sessions.lock().await;
                let index = sessions
                    .iter()
                    .position(|session| session.session_id == request.session_id)
                    .ok_or_else(|| anyhow::anyhow!("unknown session_id {}", request.session_id))?;
                let session = sessions.remove(index);
                if session.owner != request.owner {
                    anyhow::bail!("session does not belong to current user");
                }
                Ok(RunnerSessionStatus {
                    state: "cancelled".to_string(),
                    ..session
                })
            })
        }

        fn list_sessions<'a>(
            &'a self,
        ) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>> {
            Box::pin(async move { Ok(self.sessions.lock().await.clone()) })
        }

        fn run_command<'a>(
            &'a self,
            _request: RunnerCommandRequest,
        ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
            Box::pin(async move { anyhow::bail!("run_command not supported in fake") })
        }

        fn cleanup_runner_owner<'a>(
            &'a self,
            owner: RunnerOwner,
        ) -> RunnerFuture<'a, anyhow::Result<()>> {
            Box::pin(async move {
                self.sessions
                    .lock()
                    .await
                    .retain(|session| session.owner != owner);
                Ok(())
            })
        }
    }

    pub(super) fn fake_state(runner: Arc<FakeRunnerService>) -> AppState {
        AppState {
            approval: Arc::new(crate::approval::ApprovalService::disabled()),
            config: Arc::new(AppConfig {
                bind_addr: "127.0.0.1:0".to_string(),
                mcp_allowed_hosts: Vec::new(),
                workspace_root: PathBuf::from("/tmp"),
                default_shell: "bash".to_string(),
                session_idle_ttl: Duration::from_secs(60),
                max_output_bytes: 64 * 1024,
                server_scopes: ALL_SCOPES
                    .iter()
                    .map(|scope| (*scope).to_string())
                    .collect(),
                server_limits: ResourceLimits {
                    max_timeout_ms: Some(600_000),
                    max_output_bytes: Some(64 * 1024),
                    max_file_bytes: Some(50 * 1024 * 1024),
                    max_sessions: None,
                    network_enabled: true,
                },
                workspace_retention: Duration::from_secs(30 * 86_400),
                database_url: "postgres://example.invalid/test".to_string(),
                web_session_ttl: Duration::from_secs(3600),
                web_cookie_name: "desk_foreman_session".to_string(),
                web_cookie_secure: false,
                bootstrap_admin_login: None,
                bootstrap_admin_password: None,
                bootstrap_admin_display_name: None,
                bootstrap_admin_email: None,
                bootstrap_admin_timezone: "UTC".to_string(),
                frontend_dist: PathBuf::from("/tmp/frontend"),
                build_started_at: SystemTime::now(),
            }),
            runner,
            runner_broker: RunnerBroker::new(
                PgPoolOptions::new()
                    .connect_lazy("postgres://example.invalid/test")
                    .expect("lazy pool"),
            ),
            db: PgPoolOptions::new()
                .connect_lazy("postgres://example.invalid/test")
                .expect("lazy pool"),
        }
    }

    fn binding_session(session_id: u64, binding_id: i64) -> RunnerSessionStatus {
        RunnerSessionStatus {
            session_id,
            owner: RunnerOwner::WorkspaceBinding {
                workspace_binding_id: binding_id,
            },
            session_key: None,
            state: "running".to_string(),
            exit_code: None,
            timed_out: false,
            wall_time_seconds: 0.0,
        }
    }

    #[tokio::test]
    async fn cancellation_only_touches_sessions_for_target_binding() {
        let runner = FakeRunnerService::new();
        {
            let mut sessions = runner.sessions.lock().await;
            sessions.push(binding_session(1, 42));
            sessions.push(binding_session(2, 99));
        }
        let state = fake_state(runner.clone());
        let outcome = cancel_binding_sessions_best_effort(&state, 42).await;
        assert!(outcome.attempted);
        assert!(outcome.succeeded);
        assert_eq!(outcome.sessions_cancelled, 1);
        assert!(outcome.error.is_none());
        let remaining = runner.sessions.lock().await;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].session_id, 2);
    }

    #[tokio::test]
    async fn cancellation_succeeds_when_no_sessions_exist() {
        let runner = FakeRunnerService::new();
        let state = fake_state(runner.clone());
        let outcome = cancel_binding_sessions_best_effort(&state, 42).await;
        assert!(outcome.attempted);
        assert!(outcome.succeeded);
        assert_eq!(outcome.sessions_cancelled, 0);
        assert!(outcome.error.is_none());
    }

    #[tokio::test]
    async fn cancellation_records_failure_when_listing_sessions_fails() {
        struct ListFailingRunner {
            inner: Arc<FakeRunnerService>,
        }
        impl RunnerService for ListFailingRunner {
            fn exec_shell<'a>(
                &'a self,
                _request: ExecRequest,
            ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
                Box::pin(async move { anyhow::bail!("unsupported") })
            }
            fn write_stdin<'a>(
                &'a self,
                _request: InputRequest,
            ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
                Box::pin(async move { anyhow::bail!("unsupported") })
            }
            fn cancel_session<'a>(
                &'a self,
                request: CancelSessionRequest,
            ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>> {
                self.inner.cancel_session(request)
            }
            fn list_sessions<'a>(
                &'a self,
            ) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>> {
                Box::pin(async move { anyhow::bail!("runner-manager unavailable") })
            }
            fn run_command<'a>(
                &'a self,
                _request: RunnerCommandRequest,
            ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
                Box::pin(async move { anyhow::bail!("unsupported") })
            }
            fn cleanup_runner_owner<'a>(
                &'a self,
                owner: RunnerOwner,
            ) -> RunnerFuture<'a, anyhow::Result<()>> {
                self.inner.cleanup_runner_owner(owner)
            }
        }
        let inner = FakeRunnerService::new();
        let runner: Arc<dyn RunnerService> = Arc::new(ListFailingRunner {
            inner: inner.clone(),
        });
        let state = AppState {
            runner,
            ..fake_state(inner)
        };
        let outcome = cancel_binding_sessions_best_effort(&state, 42).await;
        assert!(outcome.attempted);
        assert!(!outcome.succeeded);
        assert_eq!(outcome.sessions_cancelled, 0);
        assert!(outcome.error.is_some());
        assert!(
            outcome
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("failed to list runner sessions")
        );
    }
}
