use runner_protocol::{
    CancelSessionRequest, CleanupWorkspaceBindingPrincipalsAck,
    CleanupWorkspaceBindingPrincipalsRequest, CommandOutput, EnsureExecutionEnvironmentAck,
    EnsureExecutionEnvironmentRequest, ExecRequest, InputRequest, RunnerCommandRequest,
    RunnerOwner, RunnerSessionStatus, ShellToolOutput, StopExecutionEnvironmentAck,
    StopExecutionEnvironmentRequest, StopWorkspacePrincipalAck, StopWorkspacePrincipalRequest,
    workspace_git::{PrepareWorkspaceGitRequest, PrepareWorkspaceGitResult},
};

use crate::runner::RunnerFuture;

/// Strict request for in-flight workspace activity on one exact manager.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceActivityRequest {
    pub workspace_binding_id: i64,
}

/// In-flight activity visible to a manager before a worker's Shell session
/// exists. Counts are busy indicators, not exact job counts.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceActivity {
    pub workspace_binding_id: i64,
    pub active_operations: u32,
    pub live_sessions: u32,
    pub binding_rpc_active: bool,
}

impl WorkspaceActivity {
    pub fn is_busy(&self) -> bool {
        self.active_operations > 0 || self.live_sessions > 0 || self.binding_rpc_active
    }
}

pub trait RunnerService: Send + Sync {
    fn exec_shell<'a>(
        &'a self,
        request: ExecRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>>;

    fn write_stdin<'a>(
        &'a self,
        request: InputRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>>;

    fn cancel_session<'a>(
        &'a self,
        request: CancelSessionRequest,
    ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>>;

    fn list_sessions<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>>;

    /// Observe in-flight activity for one binding on one exact manager without
    /// cancelling or otherwise mutating it. Default fails closed: it never
    /// reports a false-empty activity and never falls back to the untargeted
    /// listing.
    fn workspace_activity_for_manager<'a>(
        &'a self,
        _manager_id: i64,
        _workspace_binding_id: i64,
    ) -> RunnerFuture<'a, anyhow::Result<WorkspaceActivity>> {
        Box::pin(async {
            anyhow::bail!(
                "manager-targeted workspace activity is not supported by this runner service"
            )
        })
    }

    /// Observe local in-flight activity for one binding. Default fails closed;
    /// the local manager backend overrides it.
    fn workspace_activity<'a>(
        &'a self,
        _workspace_binding_id: i64,
    ) -> RunnerFuture<'a, anyhow::Result<WorkspaceActivity>> {
        Box::pin(async {
            anyhow::bail!("workspace activity is not supported by this runner service")
        })
    }

    fn run_command<'a>(
        &'a self,
        request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>>;

    fn cleanup_runner_owner<'a>(
        &'a self,
        owner: RunnerOwner,
    ) -> RunnerFuture<'a, anyhow::Result<()>>;

    fn cleanup_workspace_binding_principals<'a>(
        &'a self,
        _request: CleanupWorkspaceBindingPrincipalsRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CleanupWorkspaceBindingPrincipalsAck>> {
        Box::pin(async {
            anyhow::bail!(
                "workspace binding principal cleanup is not supported by this runner service"
            )
        })
    }

    fn stop_workspace_principal<'a>(
        &'a self,
        _request: StopWorkspacePrincipalRequest,
    ) -> RunnerFuture<'a, anyhow::Result<StopWorkspacePrincipalAck>> {
        Box::pin(async move {
            anyhow::bail!("workspace principal stop is not supported by this runner service")
        })
    }

    /// Install or clear automatic native Git authorization inside the ordinary
    /// workspace container. Default fails closed; the manager-backed service
    /// overrides it.
    fn prepare_workspace_git<'a>(
        &'a self,
        _request: PrepareWorkspaceGitRequest,
    ) -> RunnerFuture<'a, anyhow::Result<PrepareWorkspaceGitResult>> {
        Box::pin(async move {
            anyhow::bail!("workspace Git preparation is not supported by this runner service")
        })
    }

    /// Agent-neutral ensure. Default fails closed so existing
    /// `PullRunnerService` and test fakes keep compiling without behavior
    /// change; the manager backend overrides with the trusted-profile path.
    fn ensure_execution_environment<'a>(
        &'a self,
        _request: EnsureExecutionEnvironmentRequest,
    ) -> RunnerFuture<'a, anyhow::Result<EnsureExecutionEnvironmentAck>> {
        Box::pin(async move {
            anyhow::bail!("execution environments are not supported by this runner service")
        })
    }

    /// Agent-neutral stop with inspect-confirmed acknowledgment. Default
    /// fails closed; the manager backend overrides.
    fn stop_execution_environment<'a>(
        &'a self,
        _request: StopExecutionEnvironmentRequest,
    ) -> RunnerFuture<'a, anyhow::Result<StopExecutionEnvironmentAck>> {
        Box::pin(async move {
            anyhow::bail!("execution environments are not supported by this runner service")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runner_protocol::{
        CancelSessionRequest, CleanupWorkspaceBindingPrincipalsRequest,
        EnsureExecutionEnvironmentRequest, ExecRequest, InputRequest, RunnerCommandRequest,
        RunnerOwner,
    };

    struct DefaultService;

    impl RunnerService for DefaultService {
        fn exec_shell<'a>(
            &'a self,
            _: ExecRequest,
        ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
            Box::pin(async { anyhow::bail!("unsupported") })
        }
        fn write_stdin<'a>(
            &'a self,
            _: InputRequest,
        ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
            Box::pin(async { anyhow::bail!("unsupported") })
        }
        fn cancel_session<'a>(
            &'a self,
            _: CancelSessionRequest,
        ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>> {
            Box::pin(async { anyhow::bail!("unsupported") })
        }
        fn list_sessions<'a>(
            &'a self,
        ) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>> {
            Box::pin(async { anyhow::bail!("unsupported") })
        }
        fn run_command<'a>(
            &'a self,
            _: RunnerCommandRequest,
        ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
            Box::pin(async { anyhow::bail!("unsupported") })
        }
        fn cleanup_runner_owner<'a>(
            &'a self,
            _: RunnerOwner,
        ) -> RunnerFuture<'a, anyhow::Result<()>> {
            Box::pin(async { anyhow::bail!("unsupported") })
        }
        fn cleanup_workspace_binding_principals<'a>(
            &'a self,
            _: CleanupWorkspaceBindingPrincipalsRequest,
        ) -> RunnerFuture<'a, anyhow::Result<CleanupWorkspaceBindingPrincipalsAck>> {
            Box::pin(async { anyhow::bail!("unsupported") })
        }
        fn ensure_execution_environment<'a>(
            &'a self,
            _: EnsureExecutionEnvironmentRequest,
        ) -> RunnerFuture<'a, anyhow::Result<EnsureExecutionEnvironmentAck>> {
            Box::pin(async { anyhow::bail!("unsupported") })
        }
    }

    #[tokio::test]
    async fn workspace_activity_default_fails_closed_and_contract_is_strict() {
        // The untargeted method is part of this service's surface; the
        // manager-targeted default must still refuse rather than delegate to
        // it or report a false-empty activity.
        assert!(DefaultService.list_sessions().await.is_err());
        let error = DefaultService
            .workspace_activity_for_manager(1, 2)
            .await
            .expect_err("targeted workspace activity must fail closed by default");
        assert!(
            error
                .to_string()
                .contains("manager-targeted workspace activity is not supported"),
            "unexpected error: {error}"
        );
        assert!(DefaultService.workspace_activity(2).await.is_err());

        let request: WorkspaceActivityRequest =
            serde_json::from_value(serde_json::json!({"workspace_binding_id": 7}))
                .expect("strict request decodes");
        assert_eq!(request.workspace_binding_id, 7);
        assert!(
            serde_json::from_value::<WorkspaceActivityRequest>(
                serde_json::json!({"workspace_binding_id": 7, "extra": true})
            )
            .is_err(),
            "unknown request fields must be refused"
        );

        let idle = WorkspaceActivity {
            workspace_binding_id: 7,
            active_operations: 0,
            live_sessions: 0,
            binding_rpc_active: false,
        };
        assert!(!idle.is_busy());
        for busy in [
            WorkspaceActivity {
                active_operations: 1,
                ..idle.clone()
            },
            WorkspaceActivity {
                live_sessions: 1,
                ..idle.clone()
            },
            WorkspaceActivity {
                binding_rpc_active: true,
                ..idle.clone()
            },
        ] {
            assert!(busy.is_busy());
        }

        // A legacy manager reports a session Vec; the new control plane must
        // fail closed instead of interpreting that shape as no activity.
        assert!(serde_json::from_value::<WorkspaceActivity>(serde_json::json!([])).is_err());
    }
}
