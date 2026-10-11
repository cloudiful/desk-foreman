use runner_protocol::StartNativeTaskRequest;
use serde_json::{Value, json};

use super::{ENVIRONMENT_ID, MANAGER_ID};

pub(super) fn inspect_json(
    request: &StartNativeTaskRequest,
    task_source: &str,
    uid: u32,
    gid: u32,
) -> Value {
    let task = &request.task;
    let mut labels = serde_json::Map::new();
    labels.insert(
        "desk-foreman.owner".to_string(),
        json!(request.owner.stable_key()),
    );
    labels.insert("desk-foreman.managed".to_string(), json!(true.to_string()));
    labels.insert("desk-foreman.manager".to_string(), json!(MANAGER_ID));
    labels.insert(
        "desk-foreman.execution-environment".to_string(),
        json!(ENVIRONMENT_ID),
    );
    labels.insert("desk-foreman.generation".to_string(), json!("4"));
    labels.insert(
        "df.native_task_id".to_string(),
        json!(task.task_id.to_string()),
    );
    labels.insert(
        "df.native_binding_id".to_string(),
        json!(task.workspace_binding_id.to_string()),
    );
    labels.insert(
        "df.native_seed_sha256".to_string(),
        json!(task.manifest_sha256.clone()),
    );
    labels.insert(
        "df.native_preparation_generation".to_string(),
        json!(task.preparation_generation.to_string()),
    );
    json!({
        "Id": "native-task-container",
        "State": {"Status": "running"},
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
                "Name": "df-env-123e4567e89b12d3a456426614174000",
                "Source": "/var/lib/docker/volumes/state/_data",
                "Destination": "/state",
                "RW": true
            },
            {
                "Type": "bind",
                "Source": task_source,
                "Destination": "/workspace",
                "RW": true
            }
        ]
    })
}
