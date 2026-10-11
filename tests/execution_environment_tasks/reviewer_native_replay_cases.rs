use serde_json::{Value, json};
use uuid::Uuid;

use super::native_execution_helpers::{poll_task_job, prepare_task, stop_result};
use super::support::TaskFixture;

fn response_json(body: &[u8]) -> Value {
    serde_json::from_slice(body).expect("response JSON")
}

async fn schedule_stop(
    fixture: &TaskFixture,
    task_id: Uuid,
    generation: i64,
    token: &str,
) -> super::support::HttpOutcome {
    fixture
        .json_post(
            &fixture.application_token,
            &format!("{}/stop", fixture.task_path(task_id)),
            generation,
            json!({"expected_generation": generation, "operation_token": token}),
        )
        .await
}

async fn persisted_stop_fence(
    fixture: &TaskFixture,
    task_id: Uuid,
) -> (Uuid, Option<i64>, i64, String) {
    let (job_id, admission_generation, target_generation, state): (Uuid, Option<i64>, i64, String) =
        sqlx::query_as(
            "SELECT job.job_id, job.admission_generation, job.target_generation, job.state \
         FROM execution_environment_jobs AS job \
         JOIN execution_environment_tasks AS task ON task.stop_job_id = job.job_id \
         WHERE task.task_id = $1",
        )
        .bind(task_id)
        .fetch_one(&fixture.database.pool)
        .await
        .expect("stored linked stop job");
    (job_id, admission_generation, target_generation, state)
}

#[tokio::test]
async fn an_identical_native_stop_retry_returns_the_original_job() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    prepare_task(&fixture, task_id).await;
    let first = schedule_stop(&fixture, task_id, 4, "reviewer-stop-replay").await;
    assert_eq!(first.status, 200, "{}", first.body_text());
    let first_body = response_json(&first.body);

    let second = schedule_stop(&fixture, task_id, 4, "reviewer-stop-replay").await;
    assert_eq!(second.status, 200, "{}", second.body_text());
    let second_body = response_json(&second.body);
    let (job_id, admission_generation, target_generation, state) =
        persisted_stop_fence(&fixture, task_id).await;
    let environment_generation: (i64,) =
        sqlx::query_as("SELECT generation FROM execution_environments WHERE environment_id = $1")
            .bind(fixture.environment_id)
            .fetch_one(&fixture.database.pool)
            .await
            .expect("authorization generation");
    fixture.teardown().await;

    assert_eq!(first_body["job_id"], second_body["job_id"]);
    assert_eq!(
        Uuid::parse_str(first_body["job_id"].as_str().unwrap()).unwrap(),
        job_id
    );
    assert_eq!(admission_generation, Some(4));
    assert_eq!(target_generation, 4);
    assert_eq!(state, "pending");
    assert_eq!(environment_generation.0, 5);
}

#[tokio::test]
async fn concurrent_identical_native_stops_share_one_job_and_one_fence_bump() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    prepare_task(&fixture, task_id).await;
    let (first, second) = tokio::join!(
        schedule_stop(&fixture, task_id, 4, "concurrent-native-stop"),
        schedule_stop(&fixture, task_id, 4, "concurrent-native-stop"),
    );
    assert_eq!(first.status, 200, "{}", first.body_text());
    assert_eq!(second.status, 200, "{}", second.body_text());
    let first = response_json(&first.body);
    let second = response_json(&second.body);
    let (job_id, admission_generation, target_generation, _) =
        persisted_stop_fence(&fixture, task_id).await;
    let (generation, stop_jobs): (i64, i64) = sqlx::query_as(
        "SELECT env.generation, COUNT(job.job_id) \
         FROM execution_environments AS env \
         JOIN execution_environment_tasks AS task \
           ON task.environment_id = env.environment_id AND task.task_id = $2 \
         JOIN execution_environment_jobs AS job \
           ON job.native_task_id = task.task_id AND job.job_kind = 'stop' \
         WHERE env.environment_id = $1 \
         GROUP BY env.generation",
    )
    .bind(fixture.environment_id)
    .bind(task_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("single durable stop and one authorization bump");
    fixture.teardown().await;

    assert_eq!(first["job_id"], second["job_id"]);
    assert_eq!(
        Uuid::parse_str(first["job_id"].as_str().unwrap()).unwrap(),
        job_id
    );
    assert_eq!(admission_generation, Some(4));
    assert_eq!(target_generation, 4);
    assert_eq!(generation, 5);
    assert_eq!(stop_jobs, 1);
}

#[tokio::test]
async fn terminal_stop_replay_uses_its_admission_fence_and_rejects_a_changed_fence() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    prepare_task(&fixture, task_id).await;
    let first = schedule_stop(&fixture, task_id, 4, "terminal-stop-replay").await;
    assert_eq!(first.status, 200, "{}", first.body_text());
    let (stop_job_id, job) = poll_task_job(&fixture).await;
    assert_eq!(job["kind"], "stop_native_task");
    assert_eq!(
        fixture
            .manager_result(stop_job_id, true, stop_result(&job, true))
            .await
            .status,
        204
    );

    let replay = schedule_stop(&fixture, task_id, 4, "terminal-stop-replay").await;
    assert_eq!(replay.status, 200, "{}", replay.body_text());
    let replay_body = response_json(&replay.body);
    let changed_fence = schedule_stop(&fixture, task_id, 5, "terminal-stop-replay").await;
    let (stored_job_id, admission_generation, target_generation, state) =
        persisted_stop_fence(&fixture, task_id).await;
    fixture.teardown().await;

    assert_eq!(replay_body["job_id"], format!("{stop_job_id}"));
    assert_eq!(changed_fence.status, 409, "{}", changed_fence.body_text());
    assert_eq!(stored_job_id, stop_job_id);
    assert_eq!(admission_generation, Some(4));
    assert_eq!(target_generation, 4);
    assert_eq!(state, "succeeded");
}

#[tokio::test]
async fn a_stop_replay_without_a_recorded_admission_fence_conflicts() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    prepare_task(&fixture, task_id).await;
    let scheduled = schedule_stop(&fixture, task_id, 4, "missing-admission-fence").await;
    assert_eq!(scheduled.status, 200, "{}", scheduled.body_text());
    let descriptor = response_json(&scheduled.body);
    let job_id = Uuid::parse_str(descriptor["job_id"].as_str().expect("job id")).expect("job UUID");
    sqlx::query(
        "UPDATE execution_environment_jobs SET admission_generation = NULL WHERE job_id = $1",
    )
    .bind(job_id)
    .execute(&fixture.database.pool)
    .await
    .expect("model a historical row without the fence");

    let replay = schedule_stop(&fixture, task_id, 4, "missing-admission-fence").await;
    let (state, admission_generation): (String, Option<i64>) = sqlx::query_as(
        "SELECT state, admission_generation FROM execution_environment_jobs WHERE job_id = $1",
    )
    .bind(job_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("unchanged historical stop job");
    let environment_generation: (i64,) =
        sqlx::query_as("SELECT generation FROM execution_environments WHERE environment_id = $1")
            .bind(fixture.environment_id)
            .fetch_one(&fixture.database.pool)
            .await
            .expect("unchanged authorization generation");
    fixture.teardown().await;

    assert_eq!(replay.status, 409, "{}", replay.body_text());
    assert_eq!(state, "pending");
    assert_eq!(admission_generation, None);
    assert_eq!(environment_generation.0, 5);
}

#[tokio::test]
async fn recovery_stop_persists_and_replays_its_current_request_fence() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    prepare_task(&fixture, task_id).await;
    let initial = schedule_stop(&fixture, task_id, 4, "initial-stop").await;
    assert_eq!(initial.status, 200, "{}", initial.body_text());
    let (initial_job_id, initial_job) = poll_task_job(&fixture).await;
    assert_eq!(initial_job["kind"], "stop_native_task");
    assert_eq!(
        fixture
            .manager_result(initial_job_id, false, Value::Null)
            .await
            .status,
        204
    );

    let recovery = schedule_stop(&fixture, task_id, 5, "recovery-stop").await;
    assert_eq!(recovery.status, 200, "{}", recovery.body_text());
    let recovery_body = response_json(&recovery.body);
    let replay = schedule_stop(&fixture, task_id, 5, "recovery-stop").await;
    assert_eq!(replay.status, 200, "{}", replay.body_text());
    let replay_body = response_json(&replay.body);
    let old_fence = schedule_stop(&fixture, task_id, 4, "recovery-stop").await;
    let (job_id, admission_generation, target_generation, state) =
        persisted_stop_fence(&fixture, task_id).await;
    fixture.teardown().await;

    assert_eq!(recovery_body["job_id"], replay_body["job_id"]);
    assert_eq!(
        Uuid::parse_str(recovery_body["job_id"].as_str().unwrap()).unwrap(),
        job_id
    );
    assert_eq!(admission_generation, Some(5));
    assert_eq!(target_generation, 4);
    assert_eq!(state, "pending");
    assert_eq!(old_fence.status, 409, "{}", old_fence.body_text());
}
