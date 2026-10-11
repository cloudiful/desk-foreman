use runner_protocol::ExportNativeTaskRequest;
use serde_json::{Value, json};
use uuid::Uuid;

use super::export_helpers::{
    export_path, manager_upload, schedule_export, selected_content, stop_prepared_task, upload_for,
};
use super::native_execution_helpers::{poll_task_job, prepare_task};
use super::support::{TaskFixture, call, create_manager};

#[tokio::test]
async fn export_requires_a_positive_task_owned_quiescent_stop_proof() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    prepare_task(&fixture, task_id).await;
    assert_eq!(
        schedule_export(
            &fixture,
            task_id,
            Uuid::new_v4(),
            "no-proof",
            &["README.md"]
        )
        .await
        .status,
        409
    );
    fixture.teardown().await;

    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    let proof_id = stop_prepared_task(&fixture, task_id).await;
    assert_eq!(
        schedule_export(
            &fixture,
            task_id,
            Uuid::new_v4(),
            "foreign-proof",
            &["README.md"]
        )
        .await
        .status,
        409
    );
    let original: Value =
        sqlx::query_scalar("SELECT result FROM execution_environment_jobs WHERE job_id = $1")
            .bind(proof_id)
            .fetch_one(&fixture.database.pool)
            .await
            .unwrap();
    for (field, value) in [
        ("preparation_quiescent", json!(false)),
        ("confirmed", json!(false)),
        ("observed_state", json!("running")),
    ] {
        let mut invalid = original.clone();
        invalid[field] = value;
        sqlx::query("UPDATE execution_environment_jobs SET result = $2 WHERE job_id = $1")
            .bind(proof_id)
            .bind(invalid)
            .execute(&fixture.database.pool)
            .await
            .unwrap();
        assert_eq!(
            schedule_export(&fixture, task_id, proof_id, "bad-proof", &["README.md"])
                .await
                .status,
            409,
            "{field}"
        );
    }
    sqlx::query("UPDATE execution_environment_jobs SET result = $2 WHERE job_id = $1")
        .bind(proof_id)
        .bind(original)
        .execute(&fixture.database.pool)
        .await
        .unwrap();
    let (other_manager, _) = create_manager(&fixture.database.pool, "export-other-manager").await;
    sqlx::query("UPDATE execution_environment_jobs SET assigned_manager_id = $2 WHERE job_id = $1")
        .bind(proof_id)
        .bind(other_manager)
        .execute(&fixture.database.pool)
        .await
        .unwrap();
    assert_eq!(
        schedule_export(
            &fixture,
            task_id,
            proof_id,
            "wrong-manager-proof",
            &["README.md"]
        )
        .await
        .status,
        409
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_environment_task_exports")
        .fetch_one(&fixture.database.pool)
        .await
        .unwrap();
    fixture.teardown().await;
    assert_eq!(count, 0);
}

#[tokio::test]
async fn export_application_and_manager_identities_are_isolated() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    let proof_id = stop_prepared_task(&fixture, task_id).await;
    let foreign = call(
        &fixture.base,
        "POST",
        &export_path(&fixture, task_id, ""),
        Some(&fixture.other_application_token),
        &[],
        &serde_json::to_vec(
            &json!({"operation_token": "foreign", "stop_job_id": proof_id, "paths": ["README.md"]}),
        )
        .unwrap(),
    )
    .await;
    assert_eq!(foreign.status, 404);
    assert_eq!(
        schedule_export(&fixture, task_id, proof_id, "owned-export", &["README.md"])
            .await
            .status,
        200
    );
    for suffix in ["/status", "/content"] {
        assert_eq!(
            fixture
                .get(
                    &fixture.other_application_token,
                    &export_path(&fixture, task_id, suffix)
                )
                .await
                .status,
            404
        );
    }
    assert_eq!(
        fixture
            .get(
                &fixture.application_token,
                &export_path(&fixture, task_id, "/content")
            )
            .await
            .status,
        409
    );
    let (job_id, job) = poll_task_job(&fixture).await;
    let request: ExportNativeTaskRequest = serde_json::from_value(job["payload"].clone()).unwrap();
    let content = selected_content(&request);
    let (_, other_token) = create_manager(&fixture.database.pool, "export-foreign-uploader").await;
    assert_eq!(
        manager_upload(&fixture, job_id, b"{", Some(&other_token))
            .await
            .status,
        404
    );
    assert_eq!(
        manager_upload(&fixture, job_id, b"{", None).await.status,
        401
    );
    assert_eq!(
        manager_upload(&fixture, job_id, b"{", Some(&fixture.manager_token))
            .await
            .status,
        400
    );
    let mut stale = request.clone();
    stale.owner = runner_protocol::RunnerOwner::ExecutionEnvironment {
        environment_id: fixture.environment_id.to_string(),
        generation: 5,
    };
    assert_eq!(
        manager_upload(
            &fixture,
            job_id,
            &upload_for(stale, content),
            Some(&fixture.manager_token)
        )
        .await
        .status,
        409
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn only_new_export_scheduling_requires_an_online_original_manager() {
    let fixture = TaskFixture::setup().await;
    let task_id = Uuid::new_v4();
    let proof_id = stop_prepared_task(&fixture, task_id).await;
    sqlx::query("UPDATE runner_managers SET status = 'offline' WHERE runner_manager_id = $1")
        .bind(fixture.manager_id)
        .execute(&fixture.database.pool)
        .await
        .unwrap();
    assert_eq!(
        schedule_export(
            &fixture,
            task_id,
            proof_id,
            "offline-export",
            &["README.md"]
        )
        .await
        .status,
        503
    );
    sqlx::query("UPDATE runner_managers SET status = 'online', last_seen_at = NOW() WHERE runner_manager_id = $1")
        .bind(fixture.manager_id).execute(&fixture.database.pool).await.unwrap();
    let first = schedule_export(
        &fixture,
        task_id,
        proof_id,
        "offline-export",
        &["README.md"],
    )
    .await;
    assert_eq!(first.status, 200);
    sqlx::query("UPDATE runner_managers SET status = 'offline' WHERE runner_manager_id = $1")
        .bind(fixture.manager_id)
        .execute(&fixture.database.pool)
        .await
        .unwrap();
    let replay = schedule_export(
        &fixture,
        task_id,
        proof_id,
        "offline-export",
        &["README.md"],
    )
    .await;
    fixture.teardown().await;
    assert_eq!(replay.status, 200);
    assert_eq!(first.body, replay.body);
}
