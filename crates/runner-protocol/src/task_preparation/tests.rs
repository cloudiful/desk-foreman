//! Unit tests for the preparation DTOs (issue 753 P2b).

use serde_json::json;
use uuid::Uuid;

use super::{NativeTaskPreparedResult, PrepareNativeTaskRequest};
use crate::RunnerOwner;

#[test]
fn a_prepare_request_denies_unknown_fields() {
    let parsed: Result<PrepareNativeTaskRequest, _> = serde_json::from_value(json!({
        "owner": { "ExecutionEnvironment": { "environment_id": "env", "generation": 1 } },
        "task_id": "123e4567-e89b-12d3-a456-426614174000",
        "workspace_binding_id": 1,
        "manifest_sha256": "0".repeat(64),
        "file_count": 0,
        "total_bytes": 0,
        "host_root": "/var/lib"
    }));
    assert!(parsed.is_err(), "a host root must never be accepted");
}

#[test]
fn a_prepared_result_denies_unknown_fields() {
    let parsed: Result<NativeTaskPreparedResult, _> = serde_json::from_value(json!({
        "task_id": "123e4567-e89b-12d3-a456-426614174000",
        "manifest_sha256": "0".repeat(64),
        "file_count": 0,
        "total_bytes": 0,
        "prepared": true,
        "checkout_path": "/var/lib/tree"
    }));
    assert!(parsed.is_err(), "a checkout path must never be accepted");
}

#[test]
fn a_prepare_request_round_trips() {
    let request = PrepareNativeTaskRequest {
        owner: RunnerOwner::ExecutionEnvironment {
            environment_id: "env-1".to_string(),
            generation: 3,
        },
        task_id: Uuid::new_v4(),
        workspace_binding_id: 9,
        manifest_sha256: "a".repeat(64),
        file_count: 2,
        total_bytes: 10,
    };
    let encoded = serde_json::to_string(&request).expect("encode");
    let decoded: PrepareNativeTaskRequest = serde_json::from_str(&encoded).expect("decode");
    assert_eq!(decoded, request);
}
