use std::collections::HashMap;

use runner_protocol::{
    CancelSessionRequest, CommandOutput, ExecRequest, InputRequest, RunnerCommandRequest,
    RunnerOwner, RunnerSessionStatus, ShellToolOutput,
};

use super::{LifecycleError, ensure_no_scope_sessions_for_managers};
use crate::runner::{RunnerFuture, RunnerService, WorkspaceActivity};

const BINDING_ID: i64 = 11;

struct FakeRunner {
    activities: HashMap<i64, Result<WorkspaceActivity, ()>>,
}

impl FakeRunner {
    fn new(activities: HashMap<i64, Result<WorkspaceActivity, ()>>) -> Self {
        Self { activities }
    }
}

impl RunnerService for FakeRunner {
    fn exec_shell<'a>(
        &'a self,
        _: ExecRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async { anyhow::bail!("unused") })
    }

    fn write_stdin<'a>(
        &'a self,
        _: InputRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async { anyhow::bail!("unused") })
    }

    fn cancel_session<'a>(
        &'a self,
        _: CancelSessionRequest,
    ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>> {
        Box::pin(async { anyhow::bail!("unused") })
    }

    fn list_sessions<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>> {
        Box::pin(async { anyhow::bail!("unused") })
    }

    fn run_command<'a>(
        &'a self,
        _: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
        Box::pin(async { anyhow::bail!("unused") })
    }

    fn cleanup_runner_owner<'a>(&'a self, _: RunnerOwner) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async { anyhow::bail!("unused") })
    }

    fn workspace_activity_for_manager<'a>(
        &'a self,
        manager_id: i64,
        _workspace_binding_id: i64,
    ) -> RunnerFuture<'a, anyhow::Result<WorkspaceActivity>> {
        let response = match self.activities.get(&manager_id) {
            Some(Ok(activity)) => Ok(activity.clone()),
            Some(Err(())) | None => Err(anyhow::anyhow!("manager unavailable")),
        };
        Box::pin(async move { response })
    }
}

fn idle(binding_id: i64) -> WorkspaceActivity {
    WorkspaceActivity {
        workspace_binding_id: binding_id,
        active_operations: 0,
        live_sessions: 0,
        binding_rpc_active: false,
    }
}

async fn observe(
    activities: HashMap<i64, Result<WorkspaceActivity, ()>>,
    managers: &[i64],
) -> Result<(), LifecycleError> {
    let runner = FakeRunner::new(activities);
    ensure_no_scope_sessions_for_managers(managers, &runner, BINDING_ID).await
}

#[tokio::test]
async fn empty_manager_snapshot_fails_closed() {
    let result = observe(HashMap::new(), &[]).await;
    assert!(matches!(
        result,
        Err(LifecycleError::ScopeManagerUnavailable)
    ));
}

#[tokio::test]
async fn clean_managers_without_binding_activity_pass() {
    let mut activities = HashMap::new();
    activities.insert(1, Ok(idle(BINDING_ID)));
    activities.insert(2, Ok(idle(BINDING_ID)));
    assert!(observe(activities, &[1, 2]).await.is_ok());
}

#[tokio::test]
async fn any_busy_activity_is_busy_even_without_live_sessions() {
    for activity in [
        WorkspaceActivity {
            active_operations: 1,
            ..idle(BINDING_ID)
        },
        WorkspaceActivity {
            live_sessions: 1,
            ..idle(BINDING_ID)
        },
        WorkspaceActivity {
            binding_rpc_active: true,
            ..idle(BINDING_ID)
        },
    ] {
        let activities = HashMap::from([(1, Ok(activity))]);
        let result = observe(activities, &[1]).await;
        assert!(matches!(result, Err(LifecycleError::ScopeBusy)));
    }
}

#[tokio::test]
async fn mismatched_activity_reply_fails_closed() {
    let activities = HashMap::from([(1, Ok(idle(BINDING_ID + 1)))]);
    let result = observe(activities, &[1]).await;
    assert!(matches!(
        result,
        Err(LifecycleError::ScopeManagerUnavailable)
    ));
}

#[tokio::test]
async fn unavailable_manager_fails_closed() {
    let mut activities = HashMap::new();
    activities.insert(1, Ok(idle(BINDING_ID)));
    activities.insert(2, Err(()));
    let result = observe(activities, &[1, 2]).await;
    assert!(matches!(
        result,
        Err(LifecycleError::ScopeManagerUnavailable)
    ));

    let activities = HashMap::from([(1, Ok(idle(BINDING_ID)))]);
    let result = observe(activities, &[1, 3]).await;
    assert!(matches!(
        result,
        Err(LifecycleError::ScopeManagerUnavailable)
    ));
}
