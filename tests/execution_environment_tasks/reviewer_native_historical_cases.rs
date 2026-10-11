use desk_foreman::runner::service_relay::{StreamRequest, TunnelHandle};
use runner_protocol::service_relay::RelayId;
use serde_json::json;
use uuid::Uuid;

use super::native_execution_helpers::{poll_task_job, prepare_task, stop_result};
use super::support::TaskFixture;

#[tokio::test]
async fn historical_stop_replay_does_not_cancel_a_successor_generations_stream() {
    let fixture = TaskFixture::setup().await;
    let old_task = Uuid::new_v4();
    prepare_task(&fixture, old_task).await;
    let old_stop_path = format!("{}/stop", fixture.task_path(old_task));
    let old_request = json!({"expected_generation": 4, "operation_token": "historical-stop"});
    let scheduled = fixture
        .json_post(
            &fixture.application_token,
            &old_stop_path,
            4,
            old_request.clone(),
        )
        .await;
    assert_eq!(scheduled.status, 200, "{}", scheduled.body_text());
    let (stop_id, job) = poll_task_job(&fixture).await;
    assert_eq!(
        fixture
            .manager_result(stop_id, true, stop_result(&job, true))
            .await
            .status,
        204
    );
    assert_eq!(fixture.release(old_task, stop_id, 5, 4).await.status, 200);

    let replacement = fixture
        .json_post(
            &fixture.application_token,
            &format!(
                "/api/internal/execution-environments/{}/replace",
                fixture.environment_id
            ),
            5,
            json!({
                "expected_generation": 5, "expected_container_generation": 4,
                "expected_stop_job_id": stop_id, "operation_token": "successor-replace"
            }),
        )
        .await;
    assert_eq!(replacement.status, 200, "{}", replacement.body_text());
    let (ensure_id, job) = poll_task_job(&fixture).await;
    let owner: runner_protocol::RunnerOwner =
        serde_json::from_value(job["payload"]["owner"].clone()).unwrap();
    let ack = json!({"owner": owner, "container_name": owner.container_name(), "state": "running"});
    assert_eq!(
        fixture.manager_result(ensure_id, true, ack).await.status,
        204
    );
    let status = fixture
        .get(
            &fixture.application_token,
            &format!(
                "/api/internal/execution-environments/{}/status",
                fixture.environment_id
            ),
        )
        .await;
    assert_eq!(status.status, 200, "{}", status.body_text());
    let status: serde_json::Value = serde_json::from_slice(&status.body).unwrap();
    assert_eq!(status["state"], "ready");

    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let tunnel = TunnelHandle::new(RelayId::from_bytes(Uuid::new_v4().into_bytes()), tx);
    fixture
        .relay
        .attach_tunnel(fixture.manager_id, tunnel)
        .unwrap();
    let stream = fixture
        .relay
        .open_stream(StreamRequest {
            stream_id: RelayId::from_bytes(Uuid::new_v4().into_bytes()),
            environment_id: fixture.environment_id,
            generation: 5,
            manager_id: fixture.manager_id,
            declared_body_length: 0,
        })
        .unwrap();
    assert!(!stream.is_cancelled());
    let replay = fixture
        .json_post(&fixture.application_token, &old_stop_path, 4, old_request)
        .await;
    let cancelled = stream.is_cancelled();
    drop(stream);
    fixture.teardown().await;
    assert_eq!(replay.status, 200, "{}", replay.body_text());
    assert!(
        !cancelled,
        "historical read-only stop replay cancelled the successor stream"
    );
}
