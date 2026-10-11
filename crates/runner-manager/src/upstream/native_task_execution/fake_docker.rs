use std::path::Path;

use runner_protocol::{NativeTaskRuntimeIdentity, RunnerOwner};
use serde_json::{Value, json};

pub(super) fn inspect_value(
    owner: &RunnerOwner,
    task: &NativeTaskRuntimeIdentity,
    host_tree: &Path,
    manager_id: i64,
    uid: u32,
    gid: u32,
    status: &str,
) -> Value {
    let (environment_id, generation) =
        runner_protocol::execution_environment::as_execution_environment(owner)
            .expect("environment owner");
    let volume = runner_protocol::execution_environment::derive_state_volume_name(environment_id)
        .expect("state volume name");
    let labels = json!({
        "desk-foreman.owner": owner.stable_key(),
        "desk-foreman.managed": "true",
        "desk-foreman.manager": manager_id.to_string(),
        "desk-foreman.execution-environment": environment_id,
        "desk-foreman.generation": generation.to_string(),
        "df.native_task_id": task.task_id.to_string(),
        "df.native_binding_id": task.workspace_binding_id.to_string(),
        "df.native_seed_sha256": task.manifest_sha256,
        "df.native_preparation_generation": task.preparation_generation.to_string(),
    });
    json!({
        "Id": "native-task-container",
        "State": {"Status": status},
        "Config": {
            "Image": "test-image",
            "Labels": labels,
            "User": format!("{uid}:{gid}"),
            "WorkingDir": "/workspace"
        },
        "HostConfig": {
            "Privileged": false,
            "PublishAllPorts": false,
            "PortBindings": null
        },
        "Mounts": [
            {
                "Type": "volume",
                "Name": volume,
                "Source": "/var/lib/docker/volumes/state/_data",
                "Destination": "/state",
                "RW": true
            },
            {
                "Type": "bind",
                "Source": host_tree,
                "Destination": "/workspace",
                "RW": true
            }
        ]
    })
}
