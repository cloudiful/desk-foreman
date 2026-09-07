//! Lease-related runner session helpers.
//!
//! Focused cancellation regressions live in `lease_takeover_tests.rs` with
//! fakes in `lease_test_support.rs` to keep this file under the 400-line cap.

use runner_protocol::{CancelSessionRequest, RunnerOwner};

use crate::{AppState, db::types::WorkspaceLeaseCancellationOutcome};

/// Cancel every runner session scoped to the binding and verify quiescence.
///
/// Cancels every session whose `RunnerOwner::WorkspaceBinding { workspace_binding_id }`
/// matches `binding_id`. The helper is binding-scoped, not strictly
/// previous-owner scoped: any runner session that targets the binding is
/// cancelled because such sessions share filesystem and runner state with
/// the displaced lease.
///
/// Each `cancel_session` call is awaited, so a returned outcome reflects
/// completed cancellations rather than best-effort fire-and-forget
/// requests. After the per-session cancels the helper lists sessions again
/// and requires zero remaining sessions for the binding; leftovers (failed
/// cancels or sessions created concurrently) flip `succeeded` to false.
/// It then stops the task-owned runner container via `cleanup_runner_owner`
/// (Docker `stop`+`rm`; Direct is a no-op). Local `docker exec` CLI kills
/// alone leave the exec'd shell running inside the container, so container
/// removal is the real Docker execution stop. Workspace files survive
/// because the workspace is a host bind mount; the container is recreated
/// on demand, so retry stays safe. Any container-stop failure also flips
/// `succeeded` to false (fail closed).
///
/// Errors are surfaced via the returned [`WorkspaceLeaseCancellationOutcome`]
/// rather than propagating as `AppError`. The takeover transaction awaits
/// this helper while still holding the binding row lock and rolls back
/// without transferring the lease when `succeeded` is false, so a
/// `!succeeded` outcome means the new run must not resume and the identical
/// request is safe to retry. Returning success with a failed cancellation
/// would falsely claim safety from a best-effort cancel alone.
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
    // Quiescence verification: re-list and require zero remaining sessions
    // for the binding. A failed cancel or a session created concurrently
    // with the takeover must fail the handover rather than letting the new
    // run resume alongside old execution.
    match state.runner.list_sessions().await {
        Ok(sessions) => {
            let remaining = sessions
                .iter()
                .filter(|session| {
                    session.owner
                        == (RunnerOwner::WorkspaceBinding {
                            workspace_binding_id: binding_id,
                        })
                })
                .count();
            if remaining > 0 {
                outcome.succeeded = false;
                let message = format!(
                    "{remaining} runner session(s) still active for binding {binding_id} after cancellation"
                );
                tracing::warn!(
                    workspace_binding_id = binding_id,
                    remaining,
                    "runner sessions remain after lease takeover cancellation"
                );
                outcome.error.get_or_insert(message);
            }
        }
        Err(error) => {
            outcome.succeeded = false;
            let message = format!("failed to verify runner quiescence: {error}");
            tracing::warn!(
                workspace_binding_id = binding_id,
                error = %error,
                "failed to verify runner quiescence during lease takeover"
            );
            outcome.error.get_or_insert(message);
        }
    }
    // Real container-side stop for Docker (Direct is a no-op). Always
    // attempted once past the initial list so orphaned container execs are
    // reaped even when the session map is already empty. Fail closed: a
    // wedged daemon blocks takeover instead of reporting false quiescence.
    // The whole helper runs under the takeover row-lock timeout, so this
    // call cannot pin writers beyond that bound.
    let owner = RunnerOwner::WorkspaceBinding {
        workspace_binding_id: binding_id,
    };
    if let Err(error) = state.runner.cleanup_runner_owner(owner).await {
        outcome.succeeded = false;
        let message = format!("failed to stop runner execution for binding {binding_id}: {error}");
        tracing::warn!(
            workspace_binding_id = binding_id,
            error = %error,
            "failed to stop runner container during lease takeover"
        );
        outcome.error.get_or_insert(message);
    }
    outcome
}

// Test modules live in sibling files to keep this helper under the 400-line
// cap. Wired here (rather than in `admin.rs`, which is outside this phase's
// scope) via explicit paths so `admin.rs` stays untouched.
#[cfg(test)]
#[path = "lease_test_support.rs"]
mod lease_test_support;

#[cfg(test)]
#[path = "lease_takeover_tests.rs"]
mod lease_takeover_tests;
