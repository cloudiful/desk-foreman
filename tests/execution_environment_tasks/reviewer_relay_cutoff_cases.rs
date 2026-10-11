use desk_foreman::runner::service_relay::{RelayCall, StreamRequest, TunnelHandle};
use runner_protocol::service_relay::RelayId;
use serde_json::json;
use uuid::Uuid;

use super::native_execution_helpers::{poll_task_job, prepare_task};
use super::support::TaskFixture;

fn stream(fixture: &TaskFixture, generation: i64) -> RelayCall {
    fixture
        .relay
        .open_stream(StreamRequest {
            stream_id: RelayId::from_bytes(Uuid::new_v4().into_bytes()),
            environment_id: fixture.environment_id,
            generation,
            manager_id: fixture.manager_id,
            declared_body_length: 0,
        })
        .unwrap()
}

#[tokio::test]
async fn native_legacy_and_invalidation_callbacks_use_only_the_committed_cutoff() {
    for operation in ["native-stop", "stop", "invalidate"] {
        let fixture = TaskFixture::setup().await;
        let environment_path = format!(
            "/api/internal/execution-environments/{}",
            fixture.environment_id
        );
        let task_id = Uuid::new_v4();
        if operation == "native-stop" {
            prepare_task(&fixture, task_id).await;
        } else if operation == "stop" {
            let provision = fixture
                .json_post(
                    &fixture.application_token,
                    &format!("{environment_path}/provision"),
                    4,
                    json!({"expected_generation": 4, "operation_token": "reviewer-provision"}),
                )
                .await;
            assert_eq!(provision.status, 200, "{}", provision.body_text());
            let (job_id, job) = poll_task_job(&fixture).await;
            let owner: runner_protocol::RunnerOwner =
                serde_json::from_value(job["payload"]["owner"].clone()).unwrap();
            let ack = json!({
                "owner": owner, "container_name": owner.container_name(), "state": "running"
            });
            assert_eq!(fixture.manager_result(job_id, true, ack).await.status, 204);
            assert_eq!(
                fixture
                    .get(
                        &fixture.application_token,
                        &format!("{environment_path}/status")
                    )
                    .await
                    .status,
                200
            );
        }
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        fixture
            .relay
            .attach_tunnel(
                fixture.manager_id,
                TunnelHandle::new(RelayId::from_bytes(Uuid::new_v4().into_bytes()), tx),
            )
            .unwrap();
        let old = stream(&fixture, 4);
        let successor = stream(&fixture, 5);
        let path = if operation == "native-stop" {
            format!("{}/stop", fixture.task_path(task_id))
        } else {
            format!("{environment_path}/{operation}")
        };
        let request = |generation| {
            if operation == "invalidate" {
                json!({"expected_generation": generation})
            } else {
                json!({"expected_generation": generation, "operation_token": "reviewer-cutoff"})
            }
        };
        let refused = fixture
            .json_post(&fixture.application_token, &path, 3, request(3))
            .await;
        assert_eq!(refused.status, 409, "{}", refused.body_text());
        assert!(!old.is_cancelled());
        assert!(!successor.is_cancelled());
        let committed = fixture
            .json_post(&fixture.application_token, &path, 4, request(4))
            .await;
        assert_eq!(committed.status, 200, "{}", committed.body_text());
        assert!(old.is_cancelled(), "{operation} did not cancel generation4");
        assert!(
            !successor.is_cancelled(),
            "{operation} cancelled generation5"
        );
        assert!(successor.epoch_is_current());
        let after_callback = stream(&fixture, 5);
        assert!(after_callback.epoch_is_current());
        drop((old, successor, after_callback));
        fixture.teardown().await;
    }
}
