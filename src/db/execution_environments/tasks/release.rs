//! Native task release against a confirmed stop proof (issue 753 P1).
//!
//! Release is the only path that frees an executed task, and it accepts
//! exactly one thing: a durable stop proof that belongs to this task, ran on
//! this task's original manager, targeted this task's generation, and records
//! positive quiescence. A stale proof, a foreign environment, an ensure
//! success, an error, an unconfirmed observation or a mismatched manager leaves
//! the task untouched.
//!
//! Nothing here sets the candidate or marks a task running: P1 seeds no
//! dispatched state of its own, so a task can only be released by a fixture
//! that already proved an executed task.

use sqlx::PgPool;
use uuid::Uuid;

use super::internal;
use super::queries::{clear_environment_active_task, lock_environment, lock_task, mark_released};
use super::types::{NativeTask, NativeTaskError, TaskStopProof};
use crate::db::execution_environments::lifecycle::confirmed_stop_state;

pub(super) async fn release(
    pool: &PgPool,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
    expected_generation: i64,
    expected_container_generation: i64,
    expected_stop_job_id: Uuid,
) -> Result<NativeTask, NativeTaskError> {
    let mut tx = pool.begin().await.map_err(internal)?;

    let environment = lock_environment(&mut tx, application_id, environment_id)
        .await
        .map_err(internal)?
        .ok_or(NativeTaskError::NotFound)?;

    let task = lock_task(&mut tx, application_id, environment_id, task_id)
        .await
        .map_err(internal)?
        .ok_or(NativeTaskError::NotFound)?;

    // Every fence is checked before the terminal short-circuit, so an
    // already-released task can only be returned when the caller's fences and
    // the retained proof still match exactly. A stale generation, a wrong
    // container generation, a malformed or non-matching proof and a foreign
    // environment are all refused, whether the task is live or terminal.
    if expected_generation != environment.generation {
        return Err(NativeTaskError::StaleGeneration);
    }
    // An admitted intent has no container generation of its own; only an
    // executed task can carry the one the caller fences. A voided intent is
    // deliberately released with no candidate, so it never matches here.
    if task.container_generation != Some(expected_container_generation) {
        return Err(NativeTaskError::ProofMismatch);
    }
    if environment.container_generation != Some(expected_container_generation) {
        return Err(NativeTaskError::ProofMismatch);
    }
    // The proof consumed for this environment must be the proof named here.
    if environment.stop_proof_job_id != Some(expected_stop_job_id) {
        return Err(NativeTaskError::ProofMismatch);
    }

    let proof = find_stop_proof(
        &mut tx,
        application_id,
        environment_id,
        expected_stop_job_id,
    )
    .await
    .map_err(internal)?
    .ok_or(NativeTaskError::ProofMismatch)?;
    if !proof_matches_task(Some(&proof), &task) {
        return Err(NativeTaskError::ProofMismatch);
    }

    if task.is_released() {
        // Idempotent: an exact proof replay, under a matching fence, returns
        // the same terminal task and never mutates another owner's state. The
        // environment anchor was cleared on the first release, so it is not
        // re-checked here; every fence above already passed.
        tx.commit().await.map_err(internal)?;
        return Ok(task);
    }

    // A live task must still be the environment's active anchor; a task that
    // exists without the anchor is an inconsistency and is refused.
    if environment.active_native_task_id != Some(task.task_id) {
        return Err(NativeTaskError::NotFound);
    }

    let released = mark_released(&mut tx, application_id, environment_id, task_id)
        .await
        .map_err(internal)?
        .ok_or(NativeTaskError::ProofMismatch)?;
    clear_environment_active_task(&mut tx, application_id, environment_id, task_id)
        .await
        .map_err(internal)?;

    tx.commit().await.map_err(internal)?;
    Ok(released)
}

async fn find_stop_proof(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    application_id: i64,
    environment_id: Uuid,
    job_id: Uuid,
) -> anyhow::Result<Option<TaskStopProof>> {
    Ok(sqlx::query_file_as!(
        TaskStopProof,
        "src/sql/execution_environments/tasks/find_task_stop_proof.sql",
        job_id,
        application_id,
        environment_id,
    )
    .fetch_optional(&mut **tx)
    .await?)
}

/// Every proof fact, checked together: kind, success, the task's own manager,
/// the task's own generation, the typed native-task link, and positive
/// quiescence from the existing confirmed-stop helper.
fn proof_matches_task(proof: Option<&TaskStopProof>, task: &NativeTask) -> bool {
    let Some(proof) = proof else {
        return false;
    };
    proof.job_kind == "stop"
        && proof.state == "succeeded"
        && proof.native_task_id == Some(task.task_id)
        && proof.assigned_manager_id == task.assigned_manager_id
        && proof.target_generation == task.target_generation
        && confirmed_stop_state(proof.result.as_ref()).is_some()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::proof_matches_task;
    use crate::db::execution_environments::tasks::types::{NativeTask, TaskStopProof};

    fn task() -> NativeTask {
        NativeTask {
            task_id: Uuid::new_v4(),
            operation_token: "op".to_string(),
            environment_id: Uuid::new_v4(),
            application_id: 1,
            workspace_binding_id: 2,
            subject_external_user_id: "user".to_string(),
            target_generation: 4,
            assigned_manager_id: 7,
            state: "running".to_string(),
            container_generation: Some(4),
            prepare_job_id: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            terminal_at: None,
        }
    }

    fn proof(
        native_task_id: Option<Uuid>,
        manager_id: i64,
        generation: i64,
        state: &str,
        result: serde_json::Value,
    ) -> TaskStopProof {
        TaskStopProof {
            job_id: Uuid::new_v4(),
            environment_id: Uuid::new_v4(),
            application_id: 1,
            assigned_manager_id: manager_id,
            job_kind: "stop".to_string(),
            target_generation: generation,
            state: state.to_string(),
            native_task_id,
            result: Some(result),
        }
    }

    fn confirmed() -> serde_json::Value {
        serde_json::json!({
            "outcome": "stop_confirmed",
            "observed_state": "exited",
            "confirmed": true
        })
    }

    #[test]
    fn a_matching_confirmed_stop_releases() {
        let task = task();
        let p = proof(Some(task.task_id), 7, 4, "succeeded", confirmed());
        assert!(proof_matches_task(Some(&p), &task));
    }

    #[test]
    fn a_proof_for_another_task_or_another_task_link_never_releases() {
        let task = task();
        let other = proof(Some(Uuid::new_v4()), 7, 4, "succeeded", confirmed());
        assert!(!proof_matches_task(Some(&other), &task));
        let unlinked = proof(None, 7, 4, "succeeded", confirmed());
        assert!(!proof_matches_task(Some(&unlinked), &task));
    }

    #[test]
    fn a_wrong_manager_generation_or_failure_never_releases() {
        let task = task();
        assert!(!proof_matches_task(
            Some(&proof(Some(task.task_id), 99, 4, "succeeded", confirmed())),
            &task
        ));
        assert!(!proof_matches_task(
            Some(&proof(Some(task.task_id), 7, 99, "succeeded", confirmed())),
            &task
        ));
        assert!(!proof_matches_task(
            Some(&proof(Some(task.task_id), 7, 4, "failed", confirmed())),
            &task
        ));
        assert!(!proof_matches_task(None, &task));
    }

    #[test]
    fn an_unconfirmed_observation_never_releases() {
        let task = task();
        let unconfirmed = serde_json::json!({
            "outcome": "stop_confirmed",
            "observed_state": "exited",
            "confirmed": false
        });
        assert!(!proof_matches_task(
            Some(&proof(Some(task.task_id), 7, 4, "succeeded", unconfirmed)),
            &task
        ));
        // A missing or unparsable result is equally not a confirmation.
        let mut empty = proof(Some(task.task_id), 7, 4, "succeeded", confirmed());
        empty.result = None;
        assert!(!proof_matches_task(Some(&empty), &task));
    }

    #[test]
    fn an_ensure_success_is_never_a_stop_proof() {
        let task = task();
        let mut ensure = proof(Some(task.task_id), 7, 4, "succeeded", confirmed());
        ensure.job_kind = "ensure".to_string();
        assert!(!proof_matches_task(Some(&ensure), &task));
    }
}
