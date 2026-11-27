use super::{PendingJob, RunnerBroker};
use runner_protocol::RunnerJobResult;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::oneshot;

fn broker() -> Arc<RunnerBroker> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .expect("build lazy test pool");
    RunnerBroker::new(pool)
}

async fn register(
    broker: &Arc<RunnerBroker>,
    job_id: &str,
    manager_id: Option<i64>,
) -> oneshot::Receiver<anyhow::Result<Value>> {
    let (sender, receiver) = oneshot::channel();
    broker
        .pending
        .lock()
        .await
        .insert(job_id.to_string(), PendingJob { manager_id, sender });
    receiver
}

fn producer(job_id: &str, ok: bool, result: Value, error: Option<&str>) -> RunnerJobResult {
    let wire = RunnerJobResult {
        job_id: job_id.to_string(),
        ok,
        result: Some(result),
        error: error.map(str::to_owned),
    };
    serde_json::from_value(serde_json::to_value(&wire).expect("encode manager job result envelope"))
        .expect("decode manager job result envelope")
}

#[derive(Debug, Deserialize)]
struct ObjectOnly {
    state: String,
}

#[tokio::test]
async fn null_success_completes_and_consumers_accept_it() {
    let broker = broker();
    let receiver = register(&broker, "job", Some(4)).await;
    let wire = serde_json::to_value(RunnerJobResult {
        job_id: "job".to_string(),
        ok: true,
        result: Some(Value::Null),
        error: None,
    })
    .expect("encode manager job result envelope");
    assert_eq!(wire["result"], Value::Null);
    let decoded: RunnerJobResult =
        serde_json::from_value(wire).expect("decode manager job result envelope");
    assert!(decoded.result.is_none());
    broker
        .complete_job(4, decoded)
        .await
        .expect("successful unit result must complete");
    let value = receiver.await.expect("completion").expect("success value");
    assert_eq!(value, Value::Null);
    serde_json::from_value::<()>(value.clone()).expect("unit consumer accepts null");
    assert_eq!(
        serde_json::from_value::<Value>(value)
            .expect("value consumer accepts null, as cleanup does"),
        Value::Null
    );
    assert!(broker.pending.lock().await.is_empty());
}

#[tokio::test]
async fn typed_object_consumer_still_rejects_null() {
    let broker = broker();
    let receiver = register(&broker, "job", Some(4)).await;
    broker
        .complete_job(4, producer("job", true, Value::Null, None))
        .await
        .expect("successful unit result must complete");
    let value = receiver.await.expect("completion").expect("success value");
    assert!(serde_json::from_value::<ObjectOnly>(value).is_err());
}

#[tokio::test]
async fn object_success_roundtrips_into_typed_consumer() {
    let broker = broker();
    let receiver = register(&broker, "job", Some(4)).await;
    broker
        .complete_job(4, producer("job", true, json!({"state": "stopped"}), None))
        .await
        .expect("successful object result must complete");
    let value = receiver.await.expect("completion").expect("success value");
    let object: ObjectOnly = serde_json::from_value(value).expect("object consumer");
    assert_eq!(object.state, "stopped");
}

#[tokio::test]
async fn failed_and_invalid_envelopes_preserve_errors() {
    let broker = broker();
    for (job_id, ok, result, error, expected) in [
        (
            "failed",
            false,
            Value::Null,
            Some("manager failed"),
            "manager failed",
        ),
        (
            "contradictory",
            true,
            Value::Null,
            Some("contradictory"),
            "invalid job result",
        ),
        ("absent", false, Value::Null, None, "invalid job result"),
    ] {
        let receiver = register(&broker, job_id, Some(4)).await;
        broker
            .complete_job(4, producer(job_id, ok, result, error))
            .await
            .expect("completion is acknowledged even when the result is invalid");
        let failure = receiver
            .await
            .expect("completion")
            .expect_err("envelope must be refused");
        assert!(
            failure.to_string().contains(expected),
            "{job_id}: {failure}"
        );
    }
    assert!(broker.pending.lock().await.is_empty());
}

#[tokio::test]
async fn another_manager_cannot_complete_but_the_owner_still_can() {
    let broker = broker();
    let receiver = register(&broker, "job", Some(4)).await;
    assert!(
        broker
            .complete_job(5, producer("job", true, json!({"state": "stopped"}), None))
            .await
            .is_err()
    );
    assert!(broker.pending.lock().await.contains_key("job"));
    broker
        .complete_job(4, producer("job", true, json!({"state": "stopped"}), None))
        .await
        .expect("owning manager completes");
    let value = receiver.await.expect("completion").expect("success value");
    assert_eq!(value["state"], "stopped");
}
