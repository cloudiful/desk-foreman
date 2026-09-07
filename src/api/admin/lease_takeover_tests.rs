//! Takeover cancellation regression tests (no live runner).
//!
//! Extracted from `lease_helpers.rs` to keep each file under the 400-line
//! cap. Uses the fake runner in `lease_test_support`.

use std::sync::Arc;

use runner_protocol::{
    CancelSessionRequest, CommandOutput, ExecRequest, InputRequest, RunnerCommandRequest,
    RunnerOwner, RunnerSessionStatus, ShellToolOutput,
};

use super::{
    cancel_binding_sessions_best_effort,
    lease_test_support::{FakeRunnerService, binding_session, fake_state},
};
use crate::{
    AppState,
    runner::{RunnerFuture, RunnerService},
};

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

#[tokio::test]
async fn cancellation_fails_when_session_cancel_errors() {
    struct CancelFailingRunner {
        inner: Arc<FakeRunnerService>,
    }
    impl RunnerService for CancelFailingRunner {
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
            _request: CancelSessionRequest,
        ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>> {
            Box::pin(async move { anyhow::bail!("runner-manager rejected cancel") })
        }
        fn list_sessions<'a>(
            &'a self,
        ) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>> {
            Box::pin(async move { self.inner.list_sessions().await })
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
    {
        let mut sessions = inner.sessions.lock().await;
        sessions.push(binding_session(1, 42));
    }
    let runner: Arc<dyn RunnerService> = Arc::new(CancelFailingRunner {
        inner: inner.clone(),
    });
    let state = AppState {
        runner,
        ..fake_state(inner)
    };
    // Cancel fails, so the session remains and quiescence verification
    // must also fail: the caller must treat this as a takeover failure.
    let outcome = cancel_binding_sessions_best_effort(&state, 42).await;
    assert!(outcome.attempted);
    assert!(!outcome.succeeded);
    assert_eq!(outcome.sessions_cancelled, 0);
    assert!(outcome.error.is_some());
}

#[tokio::test]
async fn cancellation_fails_when_sessions_remain_after_cancel() {
    struct NoopCancelRunner {
        inner: Arc<FakeRunnerService>,
    }
    impl RunnerService for NoopCancelRunner {
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
            // Report success without removing the session, simulating a
            // runner that acknowledges the cancel but leaves execution
            // running. Verification must catch the leftover session.
            Box::pin(async move {
                let sessions = self.inner.sessions.lock().await;
                let session = sessions
                    .iter()
                    .find(|session| session.session_id == request.session_id)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("unknown session_id {}", request.session_id))?;
                Ok(RunnerSessionStatus {
                    state: "cancelled".to_string(),
                    ..session
                })
            })
        }
        fn list_sessions<'a>(
            &'a self,
        ) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>> {
            Box::pin(async move { self.inner.list_sessions().await })
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
    {
        let mut sessions = inner.sessions.lock().await;
        sessions.push(binding_session(1, 42));
    }
    let runner: Arc<dyn RunnerService> = Arc::new(NoopCancelRunner {
        inner: inner.clone(),
    });
    let state = AppState {
        runner,
        ..fake_state(inner)
    };
    let outcome = cancel_binding_sessions_best_effort(&state, 42).await;
    assert!(outcome.attempted);
    assert!(
        !outcome.succeeded,
        "leftover sessions must fail quiescence even when cancel reports success"
    );
    assert!(outcome.error.is_some());
    assert!(
        outcome
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("still active")
    );
}

#[tokio::test]
async fn cancellation_fails_when_container_cleanup_fails() {
    struct CleanupFailingRunner {
        inner: Arc<FakeRunnerService>,
    }
    impl RunnerService for CleanupFailingRunner {
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
            Box::pin(async move { self.inner.list_sessions().await })
        }
        fn run_command<'a>(
            &'a self,
            _request: RunnerCommandRequest,
        ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
            Box::pin(async move { anyhow::bail!("unsupported") })
        }
        fn cleanup_runner_owner<'a>(
            &'a self,
            _owner: RunnerOwner,
        ) -> RunnerFuture<'a, anyhow::Result<()>> {
            Box::pin(async move { anyhow::bail!("docker daemon unavailable") })
        }
    }
    let inner = FakeRunnerService::new();
    {
        let mut sessions = inner.sessions.lock().await;
        sessions.push(binding_session(1, 42));
    }
    let runner: Arc<dyn RunnerService> = Arc::new(CleanupFailingRunner {
        inner: inner.clone(),
    });
    let state = AppState {
        runner,
        ..fake_state(inner)
    };
    // Sessions cancel and verify empty, but container stop fails: the
    // takeover must still fail closed (Docker exec may survive).
    let outcome = cancel_binding_sessions_best_effort(&state, 42).await;
    assert!(outcome.attempted);
    assert!(
        !outcome.succeeded,
        "container-stop failure must fail closed even with zero sessions"
    );
    assert!(
        outcome
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("failed to stop runner execution")
    );
}
