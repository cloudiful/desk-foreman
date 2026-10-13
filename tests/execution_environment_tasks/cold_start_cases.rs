//! Cold native start: the public path from a fresh ensure to a bound start
//! (issue 824 P1).
//!
//! A public `ensure` creates an absent environment with **no** manager, and
//! admission pins one on the *task* alone. These cases drive exactly that cold
//! shape through the real internal HTTP surface and prove that the native start
//! binds the task's own original manager atomically — never a chosen, guessed or
//! replaced one — and that nothing about an already-bound environment changes.

use serde_json::{Value, json};
use uuid::Uuid;

use super::native_execution_helpers::{
    parse_job_id, poll_task_job, prepare_task, schedule_preparation_job, start_result,
};
use super::support::{HttpOutcome, TaskFixture, call, create_manager, set_idle_fence};

fn start_path(fixture: &TaskFixture, task_id: Uuid) -> String {
    format!("{}/start", fixture.task_path(task_id))
}

fn response_json(outcome: &HttpOutcome) -> Value {
    serde_json::from_slice(&outcome.body).expect("response JSON")
}

async fn start(fixture: &TaskFixture, task_id: Uuid, token: &str) -> HttpOutcome {
    fixture
        .json_post(
            &fixture.application_token,
            &start_path(fixture, task_id),
            4,
            json!({"expected_generation": 4, "operation_token": token}),
        )
        .await
}

/// The environment row as the start transaction leaves it.
async fn environment_row(
    fixture: &TaskFixture,
) -> (String, Option<i64>, Option<i64>, Option<String>) {
    sqlx::query_as(
        "SELECT state, assigned_manager_id, container_generation, active_native_task_id::text \
         FROM execution_environments WHERE environment_id = $1",
    )
    .bind(fixture.environment_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("read environment row")
}

async fn task_row(
    fixture: &TaskFixture,
    task_id: Uuid,
) -> (String, Option<i64>, Option<i64>, Option<Uuid>) {
    sqlx::query_as(
        "SELECT state, assigned_manager_id, container_generation, start_job_id \
         FROM execution_environment_tasks WHERE task_id = $1",
    )
    .bind(task_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("read task row")
}

async fn job_count(fixture: &TaskFixture, kind: &str) -> i64 {
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM execution_environment_jobs WHERE job_kind = $1")
            .bind(kind)
            .fetch_one(&fixture.database.pool)
            .await
            .expect("count jobs");
    count
}

#[tokio::test]
async fn cold_start_binds_the_task_original_manager_in_the_start_transaction() {
    let fixture = TaskFixture::setup_cold().await;
    let task_id = Uuid::new_v4();

    // The public ensure shape: absent, no candidate, and no manager at all.
    let (state, manager, candidate, _) = environment_row(&fixture).await;
    assert_eq!(state, "absent");
    assert_eq!(manager, None, "a cold environment starts unbound");
    assert_eq!(candidate, None);

    // Admission pins the original manager on the task alone; the environment
    // is untouched by admission, preparation and the sealed seed.
    let (job_id, job) = schedule_preparation_job(&fixture, task_id).await;
    let (state, manager, candidate, _) = task_row(&fixture, task_id).await;
    assert_eq!(state, "preparing", "the task is still preparing");
    assert_eq!(manager, Some(fixture.manager_id), "the task keeps its pin");
    assert_eq!(candidate, None);
    let payload = &job["payload"];
    let prepared = fixture
        .manager_result(
            job_id,
            true,
            json!({
                "task_id": task_id,
                "manifest_sha256": payload["manifest_sha256"],
                "file_count": payload["file_count"],
                "total_bytes": payload["total_bytes"],
                "prepared": true
            }),
        )
        .await;
    assert_eq!(prepared.status, 204, "{}", prepared.body_text());
    let (state, manager, candidate, _) = environment_row(&fixture).await;
    assert_eq!(state, "absent");
    assert_eq!(manager, None, "preparation must not assign a manager");
    assert_eq!(candidate, None);

    // The real cold start now progresses instead of conflicting.
    let scheduled = start(&fixture, task_id, "cold-start").await;
    assert_eq!(scheduled.status, 200, "{}", scheduled.body_text());
    let descriptor = response_json(&scheduled);
    assert_eq!(descriptor["task_state"], "starting");
    assert_eq!(descriptor["native_task_id"], task_id.to_string());

    let (state, manager, candidate, active) = environment_row(&fixture).await;
    assert_eq!(state, "starting");
    assert_eq!(manager, Some(fixture.manager_id), "the task's own manager");
    assert_eq!(candidate, Some(4));
    assert_eq!(active, Some(task_id.to_string()));
    let (task_state, pinned, task_candidate, start_job_id) = task_row(&fixture, task_id).await;
    assert_eq!(task_state, "starting");
    assert_eq!(pinned, Some(fixture.manager_id));
    assert_eq!(task_candidate, Some(4));
    assert_eq!(
        start_job_id,
        Some(
            descriptor["job_id"]
                .as_str()
                .and_then(|id| Uuid::parse_str(id).ok())
                .expect("job uuid")
        )
    );

    // Exactly one start job, dispatched to the task's original manager.
    assert_eq!(job_count(&fixture, "start_native_task").await, 1);
    let (start_job_id, start_job) = poll_task_job(&fixture).await;
    assert_eq!(start_job["kind"], "start_native_task");
    assert_eq!(parse_job_id(&start_job), start_job_id);
    assert_eq!(
        fixture
            .manager_result(start_job_id, true, start_result(&start_job))
            .await
            .status,
        204
    );
    let (task_state, _, _, _) = task_row(&fixture, task_id).await;
    assert_eq!(task_state, "running");

    fixture.teardown().await;
}

#[tokio::test]
async fn a_cold_start_replay_returns_the_same_job_without_a_second_binding() {
    let fixture = TaskFixture::setup_cold().await;
    let task_id = Uuid::new_v4();
    prepare_task(&fixture, task_id).await;

    let first = start(&fixture, task_id, "cold-replay").await;
    assert_eq!(first.status, 200, "{}", first.body_text());
    let second = start(&fixture, task_id, "cold-replay").await;
    assert_eq!(second.status, 200, "{}", second.body_text());

    let first_descriptor = response_json(&first);
    let second_descriptor = response_json(&second);
    assert_eq!(first_descriptor["job_id"], second_descriptor["job_id"]);
    assert_eq!(
        first_descriptor["environment_id"],
        second_descriptor["environment_id"]
    );
    assert_eq!(job_count(&fixture, "start_native_task").await, 1);
    let (_, manager, _, _) = environment_row(&fixture).await;
    assert_eq!(manager, Some(fixture.manager_id));

    fixture.teardown().await;
}

#[tokio::test]
async fn a_foreign_bound_manager_is_never_retargeted() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    prepare_task(&fixture, task_id).await;
    let (other_manager_id, _other_token) =
        create_manager(&fixture.database.pool, "task-foreign-manager").await;

    // The environment already names a different manager.
    set_idle_fence(
        &fixture.database.pool,
        fixture.environment_id,
        other_manager_id,
        4,
    )
    .await;
    let before = environment_row(&fixture).await;
    let task_before = task_row(&fixture, task_id).await;

    let refused = start(&fixture, task_id, "foreign-manager").await;
    assert_eq!(refused.status, 409, "{}", refused.body_text());

    assert_eq!(environment_row(&fixture).await, before);
    assert_eq!(task_row(&fixture, task_id).await, task_before);
    assert_eq!(job_count(&fixture, "start_native_task").await, 0);

    fixture.teardown().await;
}

#[tokio::test]
async fn an_unbound_environment_outside_the_cold_shape_is_refused() {
    let fixture = TaskFixture::setup_cold().await;
    let task_id = Uuid::new_v4();
    prepare_task(&fixture, task_id).await;

    // NULL manager is only ever bindable from the exact absent shape.
    sqlx::query("UPDATE execution_environments SET state = 'ready' WHERE environment_id = $1")
        .bind(fixture.environment_id)
        .execute(&fixture.database.pool)
        .await
        .expect("move the environment out of the cold shape");

    let refused = start(&fixture, task_id, "non-cold-null").await;
    assert_eq!(refused.status, 409, "{}", refused.body_text());
    let (state, manager, _, _) = environment_row(&fixture).await;
    assert_eq!(state, "ready", "a refused start writes nothing");
    assert_eq!(manager, None);
    assert_eq!(task_row(&fixture, task_id).await.0, "prepared");
    assert_eq!(job_count(&fixture, "start_native_task").await, 0);

    fixture.teardown().await;
}

#[tokio::test]
async fn a_refused_environment_update_rolls_back_the_job_and_the_task() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    prepare_task(&fixture, task_id).await;

    // The environment already names a different manager, so the start refuses
    // on the binding check itself, before it writes anything.
    let (foreign_id, _token) =
        create_manager(&fixture.database.pool, "task-rollback-manager").await;
    set_idle_fence(
        &fixture.database.pool,
        fixture.environment_id,
        foreign_id,
        4,
    )
    .await;

    let refused = start(&fixture, task_id, "rollback-start").await;
    assert_eq!(refused.status, 409, "{}", refused.body_text());

    // Nothing was written in this shape: no job and no task transition exist,
    // and the foreign binding is exactly as it was. A start that *has* already
    // written both is covered by the cancellation case, which parks the real
    // transaction on a private gate and then aborts it.
    assert_eq!(job_count(&fixture, "start_native_task").await, 0);
    let (task_state, pinned, _, start_job_id) = task_row(&fixture, task_id).await;
    assert_eq!(task_state, "prepared");
    assert_eq!(start_job_id, None);
    assert_eq!(pinned, Some(fixture.manager_id));
    let (state, manager, candidate, _) = environment_row(&fixture).await;
    assert_eq!(state, "absent");
    assert_eq!(manager, Some(foreign_id), "never retargeted by a refusal");
    assert_eq!(candidate, None);

    fixture.teardown().await;
}

#[tokio::test]
async fn a_failed_cold_environment_update_rolls_everything_back() {
    let fixture = TaskFixture::setup_cold().await;
    let task_id = Uuid::new_v4();
    prepare_task(&fixture, task_id).await;

    // A private fault in this test's own database: the environment update
    // raises after the job and task updates already succeeded.
    sqlx::raw_sql(sqlx::AssertSqlSafe(
        "CREATE FUNCTION df_cold_start_fault() RETURNS trigger AS $body$ \
         BEGIN RAISE EXCEPTION 'forced environment update failure'; END; \
         $body$ LANGUAGE plpgsql; \
         CREATE TRIGGER df_cold_start_fault_trigger BEFORE UPDATE ON execution_environments \
         FOR EACH ROW WHEN (NEW.state = 'starting' AND NEW.container_generation IS NOT NULL) \
         EXECUTE FUNCTION df_cold_start_fault();"
            .to_string(),
    ))
    .execute(&fixture.database.pool)
    .await
    .expect("install the private fault trigger");

    let failed = start(&fixture, task_id, "fault-start").await;
    assert_eq!(failed.status, 500, "{}", failed.body_text());

    assert_eq!(job_count(&fixture, "start_native_task").await, 0);
    let (task_state, pinned, _, start_job_id) = task_row(&fixture, task_id).await;
    assert_eq!(task_state, "prepared", "the task transition rolled back");
    assert_eq!(start_job_id, None);
    assert_eq!(pinned, Some(fixture.manager_id));
    let (state, manager, candidate, _) = environment_row(&fixture).await;
    assert_eq!(state, "absent");
    assert_eq!(manager, None, "the manager binding rolled back");
    assert_eq!(candidate, None);

    fixture.teardown().await;
}

#[tokio::test]
async fn an_unusable_original_manager_fails_without_picking_another() {
    let fixture = TaskFixture::setup_cold().await;
    let task_id = Uuid::new_v4();
    prepare_task(&fixture, task_id).await;
    let (_other_id, _other_token) =
        create_manager(&fixture.database.pool, "task-standby-manager").await;

    sqlx::query("UPDATE runner_managers SET enabled = FALSE WHERE runner_manager_id = $1")
        .bind(fixture.manager_id)
        .execute(&fixture.database.pool)
        .await
        .expect("disable the original manager");

    let unavailable = start(&fixture, task_id, "offline-manager").await;
    assert_eq!(unavailable.status, 503, "{}", unavailable.body_text());

    assert_eq!(job_count(&fixture, "start_native_task").await, 0);
    assert_eq!(task_row(&fixture, task_id).await.0, "prepared");
    let (_, manager, _, _) = environment_row(&fixture).await;
    assert_eq!(
        manager, None,
        "no environment binding without a usable manager"
    );

    fixture.teardown().await;
}

#[tokio::test]
async fn concurrent_cold_starts_converge_on_one_job_and_one_binding() {
    let fixture = TaskFixture::setup_cold().await;
    let task_id = Uuid::new_v4();
    prepare_task(&fixture, task_id).await;

    let (first, second) = tokio::join!(
        start(&fixture, task_id, "cold-race"),
        start(&fixture, task_id, "cold-race"),
    );
    assert_eq!(first.status, 200, "{}", first.body_text());
    assert!(
        second.status == 200 || second.status == 409,
        "a racing start may replay or conflict, never invent a job: {}",
        second.body_text()
    );
    if second.status == 200 {
        assert_eq!(
            response_json(&first)["job_id"],
            response_json(&second)["job_id"]
        );
    }

    assert_eq!(job_count(&fixture, "start_native_task").await, 1);
    let (_, manager, _, _) = environment_row(&fixture).await;
    assert_eq!(manager, Some(fixture.manager_id), "assigned exactly once");

    fixture.teardown().await;
}

#[tokio::test]
async fn a_cold_start_never_accepts_a_foreign_manager_result() {
    let fixture = TaskFixture::setup_cold().await;
    let task_id = Uuid::new_v4();
    prepare_task(&fixture, task_id).await;
    let (_other_id, other_token) =
        create_manager(&fixture.database.pool, "task-foreign-manager").await;

    let scheduled = start(&fixture, task_id, "cold-foreign-result").await;
    assert_eq!(scheduled.status, 200, "{}", scheduled.body_text());
    let (start_job_id, start_job) = poll_task_job(&fixture).await;
    assert_eq!(start_job["kind"], "start_native_task");

    let foreign = call(
        &fixture.base,
        "POST",
        "/api/internal/runner-manager/jobs/result",
        Some(&other_token),
        &[],
        serde_json::to_vec(&json!({
            "job_id": start_job["job_id"],
            "ok": true,
            "result": start_result(&start_job),
            "error": null
        }))
        .expect("result body")
        .as_slice(),
    )
    .await;
    assert_eq!(
        foreign.status, 403,
        "a foreign manager may never complete a start"
    );

    let (task_state, _, _, task_start_job_id) = task_row(&fixture, task_id).await;
    assert_ne!(task_state, "running");
    // The foreign refusal changed nothing: the task still points at the very
    // same start job the original manager polled before the foreign result, and
    // that job is still claimed by — and dispatched to — that same manager.
    assert_eq!(task_start_job_id, Some(start_job_id));
    let (job_state, job_manager_id): (String, i64) = sqlx::query_as(
        "SELECT state, assigned_manager_id FROM execution_environment_jobs WHERE job_id = $1",
    )
    .bind(start_job_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("read the start job");
    assert_eq!(job_state, "claimed");
    assert_eq!(job_manager_id, fixture.manager_id);

    fixture.teardown().await;
}

/// The sealed seed header the prepare acknowledgment must echo.
async fn seed_header(fixture: &TaskFixture, task_id: Uuid) -> (String, i64, i64) {
    sqlx::query_as(
        "SELECT manifest_sha256, file_count, total_bytes \
         FROM execution_environment_task_seeds WHERE task_id = $1",
    )
    .bind(task_id)
    .fetch_one(&fixture.database.pool)
    .await
    .expect("read seed header")
}

async fn report_prepare_ack(
    fixture: &TaskFixture,
    job_id: Uuid,
    task_id: Uuid,
    manifest_sha256: &str,
) -> HttpOutcome {
    fixture
        .manager_result(
            job_id,
            true,
            json!({
                "task_id": task_id,
                "manifest_sha256": manifest_sha256,
                "file_count": 1,
                "total_bytes": 4,
                "prepared": true
            }),
        )
        .await
}

#[tokio::test]
async fn a_cold_prepare_ack_succeeds_and_still_leaves_the_environment_unbound() {
    let fixture = TaskFixture::setup_cold().await;
    let task_id = Uuid::new_v4();

    let (state, manager, candidate, active) = environment_row(&fixture).await;
    assert_eq!(state, "absent");
    assert_eq!(manager, None, "cold before preparation");
    assert_eq!(candidate, None);
    assert_eq!(active, None);

    let (job_id, job) = schedule_preparation_job(&fixture, task_id).await;
    let payload = job["payload"].clone();
    assert_eq!(job["kind"], "prepare_native_task");
    let (_state, pinned, _, _) = task_row(&fixture, task_id).await;
    assert_eq!(pinned, Some(fixture.manager_id));

    let ack = fixture
        .manager_result(
            job_id,
            true,
            json!({
                "task_id": task_id,
                "manifest_sha256": payload["manifest_sha256"],
                "file_count": payload["file_count"],
                "total_bytes": payload["total_bytes"],
                "prepared": true
            }),
        )
        .await;
    assert_eq!(ack.status, 204, "{}", ack.body_text());

    let (task_state, pinned, task_candidate, _) = task_row(&fixture, task_id).await;
    assert_eq!(task_state, "prepared");
    assert_eq!(pinned, Some(fixture.manager_id));
    assert_eq!(task_candidate, None);

    // The positive cold acknowledgment completes preparation without ever
    // assigning the environment. Only the native start may bind it.
    let (state, manager, candidate, active) = environment_row(&fixture).await;
    assert_eq!(state, "absent");
    assert_eq!(manager, None, "a preparation never binds the environment");
    assert_eq!(candidate, None);
    assert_eq!(active, Some(task_id.to_string()));
    assert_eq!(job_count(&fixture, "start_native_task").await, 0);

    fixture.teardown().await;
}

#[tokio::test]
async fn a_foreign_manager_never_completes_a_cold_preparation() {
    let fixture = TaskFixture::setup_cold().await;
    let task_id = Uuid::new_v4();
    let (job_id, _) = schedule_preparation_job(&fixture, task_id).await;
    let (_foreign_id, foreign_token) =
        create_manager(&fixture.database.pool, "task-ack-foreign").await;

    let refused = call(
        &fixture.base,
        "POST",
        "/api/internal/runner-manager/jobs/result",
        Some(&foreign_token),
        &[],
        serde_json::to_vec(&json!({
            "job_id": format!("envjob:{job_id}"),
            "ok": true,
            "result": json!({
                "task_id": task_id,
                "manifest_sha256": "a".repeat(64),
                "file_count": 1,
                "total_bytes": 4,
                "prepared": true
            }),
            "error": null
        }))
        .expect("result body")
        .as_slice(),
    )
    .await;
    assert_eq!(refused.status, 403, "{}", refused.body_text());

    let (state, manager, _, _) = environment_row(&fixture).await;
    assert_eq!(state, "absent");
    assert_eq!(manager, None, "a foreign ack never binds");
    assert_eq!(task_row(&fixture, task_id).await.0, "preparing");

    fixture.teardown().await;
}

#[tokio::test]
async fn a_cold_prepare_ack_is_refused_once_the_environment_leaves_the_cold_shape() {
    let fixture = TaskFixture::setup_cold().await;
    let task_id = Uuid::new_v4();
    let (job_id, _) = schedule_preparation_job(&fixture, task_id).await;
    let (manifest, _, _) = seed_header(&fixture, task_id).await;

    // A NULL manager outside the absent shape is no longer a cold environment.
    sqlx::query("UPDATE execution_environments SET state = 'ready' WHERE environment_id = $1")
        .bind(fixture.environment_id)
        .execute(&fixture.database.pool)
        .await
        .expect("move the environment out of the cold shape");

    let refused = report_prepare_ack(&fixture, job_id, task_id, &manifest).await;
    assert_eq!(refused.status, 409, "{}", refused.body_text());
    assert_eq!(task_row(&fixture, task_id).await.0, "preparing");
    let (state, manager, _, _) = environment_row(&fixture).await;
    assert_eq!(state, "ready");
    assert_eq!(manager, None);

    fixture.teardown().await;
}

#[tokio::test]
async fn a_cold_prepare_ack_is_refused_once_the_environment_has_a_candidate() {
    let fixture = TaskFixture::setup_cold().await;
    let task_id = Uuid::new_v4();
    let (job_id, _) = schedule_preparation_job(&fixture, task_id).await;
    let (manifest, _, _) = seed_header(&fixture, task_id).await;

    // Even the exact original manager is refused once a candidate exists:
    // the no-candidate rule is unchanged for the bound branch too.
    sqlx::query(
        "UPDATE execution_environments SET assigned_manager_id = $2, container_generation = 3 \
         WHERE environment_id = $1",
    )
    .bind(fixture.environment_id)
    .bind(fixture.manager_id)
    .execute(&fixture.database.pool)
    .await
    .expect("give the environment a candidate");

    let refused = report_prepare_ack(&fixture, job_id, task_id, &manifest).await;
    assert_eq!(refused.status, 409, "{}", refused.body_text());
    assert_eq!(task_row(&fixture, task_id).await.0, "preparing");

    fixture.teardown().await;
}

#[tokio::test]
async fn a_cold_prepare_ack_with_a_mismatched_seed_never_completes_or_binds() {
    let fixture = TaskFixture::setup_cold().await;
    let task_id = Uuid::new_v4();
    let (job_id, _) = schedule_preparation_job(&fixture, task_id).await;
    let (manifest, _, _) = seed_header(&fixture, task_id).await;
    assert_ne!(manifest, "0".repeat(64));

    let refused = report_prepare_ack(&fixture, job_id, task_id, &"0".repeat(64)).await;
    assert_eq!(refused.status, 400, "{}", refused.body_text());

    let (state, manager, candidate, _) = environment_row(&fixture).await;
    assert_eq!(state, "absent");
    assert_eq!(manager, None, "a mismatched seed never binds");
    assert_eq!(candidate, None);
    assert_eq!(task_row(&fixture, task_id).await.0, "preparing");
    assert_eq!(job_count(&fixture, "start_native_task").await, 0);

    fixture.teardown().await;
}

#[tokio::test]
async fn a_cold_preparation_is_never_scheduled_onto_an_unusable_manager() {
    let fixture = TaskFixture::setup_cold().await;
    let task_id = Uuid::new_v4();
    assert_eq!(
        fixture.admit_default(task_id, "cold-admit").await.status,
        200
    );
    assert_eq!(
        fixture
            .seed(
                &fixture.application_token,
                task_id,
                4,
                &[("README.md".to_string(), "seed".to_string())],
            )
            .await
            .status,
        200
    );

    sqlx::query("UPDATE runner_managers SET enabled = FALSE WHERE runner_manager_id = $1")
        .bind(fixture.manager_id)
        .execute(&fixture.database.pool)
        .await
        .expect("disable the only manager");

    let refused = fixture
        .json_post(
            &fixture.application_token,
            &fixture.prepare_path(task_id),
            4,
            json!({"expected_generation": 4, "operation_token": "cold-offline-prepare"}),
        )
        .await;
    assert_eq!(refused.status, 503, "{}", refused.body_text());

    let (state, manager, _, _) = environment_row(&fixture).await;
    assert_eq!(state, "absent");
    assert_eq!(manager, None, "an offline manager is never substituted");
    assert_eq!(task_row(&fixture, task_id).await.0, "admitted");

    fixture.teardown().await;
}
