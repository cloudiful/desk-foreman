use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use desk_foreman::runner::RunnerFuture;
use runner_protocol::{
    CommandOutput, RunnerCommandRequest, RunnerJob, RunnerOwner, RunnerShellRequest,
};
use serde_json::json;

use super::{job_activity_owner, preregister_job_activity};
use crate::runtime::{ProcessSpawnTarget, RunnerBackend};

#[derive(Default)]
struct FakeBackend {
    active: Mutex<HashMap<RunnerOwner, usize>>,
}

impl FakeBackend {
    fn count(&self, binding_id: i64) -> usize {
        self.active
            .lock()
            .expect("fake active map")
            .iter()
            .filter(|(owner, _)| owner_binding_id(owner) == Some(binding_id))
            .map(|(_, count)| *count)
            .sum()
    }
}

impl RunnerBackend for FakeBackend {
    fn reconcile<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async { anyhow::bail!("unused") })
    }

    fn prepare_shell_spawn<'a>(
        &'a self,
        _request: RunnerShellRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ProcessSpawnTarget>> {
        Box::pin(async { anyhow::bail!("unused") })
    }

    fn run_command<'a>(
        &'a self,
        _request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
        Box::pin(async { anyhow::bail!("unused") })
    }

    fn cleanup_runner_owner<'a>(
        &'a self,
        _owner: RunnerOwner,
    ) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async { anyhow::bail!("unused") })
    }

    fn begin_shell_operation(&self, owner: &RunnerOwner) {
        *self
            .active
            .lock()
            .expect("fake active map")
            .entry(owner.clone())
            .or_default() += 1;
    }

    fn end_shell_operation(&self, owner: &RunnerOwner) {
        let mut active = self.active.lock().expect("fake active map");
        if let Some(count) = active.get_mut(owner) {
            *count -= 1;
            if *count == 0 {
                active.remove(owner);
            }
        }
    }
}

fn owner_binding_id(owner: &RunnerOwner) -> Option<i64> {
    match owner {
        RunnerOwner::WorkspaceBinding {
            workspace_binding_id,
        }
        | RunnerOwner::ApplicationPrincipal {
            workspace_binding_id,
            ..
        } => Some(*workspace_binding_id),
        _ => None,
    }
}

fn exec_payload(owner: RunnerOwner) -> serde_json::Value {
    json!({
        "owner": owner,
        "session_key": null,
        "workspace_root": "/tmp",
        "cmd": "true",
        "workdir": null,
        "shell": "sh",
        "login": false,
        "tty": false,
        "timeout_ms": null,
        "yield_time_ms": null,
        "max_output_tokens": null,
        "max_output_bytes": null,
        "network_enabled": false
    })
}

fn command_payload(owner: RunnerOwner) -> serde_json::Value {
    json!({
        "owner": owner,
        "workspace_root": "/tmp",
        "working_dir": "/tmp",
        "program": "true",
        "args": [],
        "timeout_ms": null,
        "max_output_bytes": null,
        "network_enabled": false
    })
}

fn job(kind: &str, payload: serde_json::Value) -> RunnerJob {
    RunnerJob {
        job_id: "job-1".to_string(),
        kind: kind.to_string(),
        payload,
    }
}

#[test]
fn job_activity_owner_maps_only_binding_exec_and_run_kinds() {
    let binding = RunnerOwner::WorkspaceBinding {
        workspace_binding_id: 5,
    };
    let principal = RunnerOwner::ApplicationPrincipal {
        application_id: 3,
        workspace_binding_id: 5,
        principal_digest: "a".repeat(64),
    };
    assert_eq!(
        job_activity_owner("exec_shell", &exec_payload(binding.clone())),
        Some(binding.clone())
    );
    assert_eq!(
        job_activity_owner("run_command", &command_payload(principal.clone())),
        Some(principal)
    );

    for (kind, payload) in [
        (
            "exec_shell",
            exec_payload(RunnerOwner::InternalUser { user_id: 1 }),
        ),
        (
            "exec_shell",
            exec_payload(RunnerOwner::ExecutionEnvironment {
                environment_id: "123e4567-e89b-12d3-a456-426614174000".to_string(),
                generation: 1,
            }),
        ),
        (
            "cleanup_runner_owner",
            json!({ "WorkspaceBinding": { "workspace_binding_id": 5 } }),
        ),
        ("stop_workspace_principal", exec_payload(binding.clone())),
        (
            "exec_shell",
            exec_payload(RunnerOwner::WorkspaceBinding {
                workspace_binding_id: 0,
            }),
        ),
        ("exec_shell", json!({"owner": "malformed"})),
        ("run_command", serde_json::Value::Null),
    ] {
        assert!(
            job_activity_owner(kind, &payload).is_none(),
            "{kind} must not preregister activity"
        );
    }
}

#[test]
fn activity_lease_is_visible_before_any_worker_future_and_drops_with_it() {
    let backend = Arc::new(FakeBackend::default());
    let dyn_backend: Arc<dyn RunnerBackend> = backend.clone();
    let exec_job = job(
        "exec_shell",
        exec_payload(RunnerOwner::WorkspaceBinding {
            workspace_binding_id: 5,
        }),
    );

    let lease = preregister_job_activity(&dyn_backend, &exec_job).expect("activity lease");
    assert_eq!(
        backend.count(5),
        1,
        "registration must be visible synchronously, before any worker future is polled"
    );
    drop(lease);
    assert_eq!(
        backend.count(5),
        0,
        "dropping the worker lease must release the registration"
    );

    // Other and malformed kinds never register.
    let ignored = job(
        "cleanup_runner_owner",
        json!({ "WorkspaceBinding": { "workspace_binding_id": 5 } }),
    );
    assert!(preregister_job_activity(&dyn_backend, &ignored).is_none());
    assert_eq!(backend.count(5), 0);
}
