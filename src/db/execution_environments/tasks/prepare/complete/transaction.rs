//! Lock-ordered preparation completion transactions (issue 753 P3r).

use runner_protocol::NativeTaskPreparedResult;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::{
    PreparationCompletion, PreparationCompletionOutcome, mark_preparation_uncertain,
    writes::{complete_job, mark_task_prepared},
};
use crate::db::execution_environment_jobs::{
    ExecutionEnvironmentJobResult, find_execution_environment_job_by_id,
};
use crate::db::execution_environments::tasks::{
    queries::{lock_binding, lock_environment, lock_task},
    seeds::find_native_task_seed_identity,
};

struct LockedPreparation<'a> {
    tx: Transaction<'a, Postgres>,
    application_id: i64,
    environment_id: Uuid,
    task_id: Uuid,
    job_id: Uuid,
}

enum LockAttempt<'a> {
    Locked(LockedPreparation<'a>),
    Refused(PreparationCompletionOutcome),
}

/// Lock and revalidate every current owner fact in environment/task/binding/job
/// order. The first job read is only a routing hint; all decisions use the
/// locked rows after any wait.
async fn lock_current<'a>(
    pool: &'a PgPool,
    manager_id: i64,
    job_id: Uuid,
) -> anyhow::Result<LockAttempt<'a>> {
    let Some(peek) = find_execution_environment_job_by_id(pool, job_id).await? else {
        return Ok(LockAttempt::Refused(PreparationCompletionOutcome::NotFound));
    };
    if peek.assigned_manager_id != manager_id {
        return Ok(LockAttempt::Refused(
            PreparationCompletionOutcome::ForeignManager,
        ));
    }
    if peek.job_kind != "prepare_native_task" {
        return Ok(LockAttempt::Refused(
            PreparationCompletionOutcome::NotCompletable,
        ));
    }
    let Some(task_id) = peek.native_task_id else {
        return Ok(LockAttempt::Refused(
            PreparationCompletionOutcome::Inconsistent("preparation job carries no native task"),
        ));
    };

    let mut tx = pool.begin().await?;
    let Some(environment) =
        lock_environment(&mut tx, peek.application_id, peek.environment_id).await?
    else {
        return Ok(LockAttempt::Refused(PreparationCompletionOutcome::NotFound));
    };
    let Some(task) = lock_task(&mut tx, peek.application_id, peek.environment_id, task_id).await?
    else {
        return Ok(LockAttempt::Refused(
            PreparationCompletionOutcome::NotCompletable,
        ));
    };
    let Some(binding) = lock_binding(&mut tx, task.workspace_binding_id).await? else {
        return Ok(LockAttempt::Refused(
            PreparationCompletionOutcome::NotCompletable,
        ));
    };
    let Some(job) = sqlx::query_file!(
        "src/sql/execution_environments/tasks/prepare/lock_preparation_job.sql",
        job_id,
        peek.application_id,
        peek.environment_id,
    )
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(LockAttempt::Refused(
            PreparationCompletionOutcome::NotCompletable,
        ));
    };

    if job.assigned_manager_id != manager_id {
        return Ok(LockAttempt::Refused(
            PreparationCompletionOutcome::ForeignManager,
        ));
    }
    let owner_matches = job.job_id == job_id
        && job.job_kind == "prepare_native_task"
        && job.native_task_id == Some(task_id)
        && job.state == "claimed"
        && job.deadline_valid.unwrap_or(false)
        && job.target_generation == environment.generation
        && job.target_generation == task.target_generation
        && environment.active_native_task_id == Some(task_id)
        && environment_accepts_preparation_binding(&environment, manager_id)
        && task.assigned_manager_id == manager_id
        && task.state == "preparing"
        && task.prepare_job_id == Some(job_id)
        && task.subject_external_user_id == environment.external_user_id
        && binding.application_id == peek.application_id
        && binding.is_active_code_project()
        && binding.lifecycle_state == "active";
    if !owner_matches {
        return Ok(LockAttempt::Refused(
            PreparationCompletionOutcome::NotCompletable,
        ));
    }

    Ok(LockAttempt::Locked(LockedPreparation {
        tx,
        application_id: peek.application_id,
        environment_id: peek.environment_id,
        task_id,
        job_id,
    }))
}

/// Whether this environment may accept the original manager's preparation proof.
///
/// A cold environment — no manager assigned yet, still absent and with no
/// candidate — accepts the proof of the manager the task itself is pinned to.
/// Every other shape keeps the original rule: the environment must already
/// name this exact manager, and it must carry no candidate generation in either
/// branch. Completing a preparation never assigns a manager; the environment
/// stays unbound until the native start binds the task's own pin atomically.
fn environment_accepts_preparation_binding(
    environment: &crate::db::execution_environments::tasks::TaskEnvironment,
    manager_id: i64,
) -> bool {
    if environment.container_generation.is_some() {
        return false;
    }
    match environment.assigned_manager_id {
        Some(assigned_manager_id) => assigned_manager_id == manager_id,
        None => environment.state == "absent",
    }
}

/// Validate the proof, then record job success and `prepared` atomically.
pub(super) async fn complete(
    pool: &PgPool,
    completion: PreparationCompletion,
) -> anyhow::Result<PreparationCompletionOutcome> {
    if completion.failed {
        return fail(pool, completion.manager_id, completion.job_id).await;
    }
    let Some(ack) = completion.result.as_ref() else {
        return Ok(PreparationCompletionOutcome::Inconsistent(
            "prepared result carried no payload",
        ));
    };
    let mut locked = match lock_current(pool, completion.manager_id, completion.job_id).await? {
        LockAttempt::Locked(locked) => locked,
        LockAttempt::Refused(outcome) => return Ok(outcome),
    };

    let Some((seed_hash, seed_count, seed_bytes)) =
        find_native_task_seed_identity(&mut locked.tx, locked.task_id).await?
    else {
        locked.tx.rollback().await?;
        return Ok(PreparationCompletionOutcome::Inconsistent(
            "preparation task has no sealed seed",
        ));
    };
    if !ack_matches(ack, locked.task_id, &seed_hash, seed_count, seed_bytes) {
        locked.tx.rollback().await?;
        return Ok(PreparationCompletionOutcome::Inconsistent(
            "prepared ack does not match the sealed seed",
        ));
    }

    let result = serde_json::to_value(ExecutionEnvironmentJobResult {
        outcome: "task_prepared".to_string(),
        observed_state: None,
        confirmed: None,
        preparation_quiescent: None,
    })?;
    let completed = complete_job(
        &mut locked.tx,
        locked.job_id,
        locked.application_id,
        locked.environment_id,
        locked.task_id,
        completion.manager_id,
        "succeeded",
        result,
        None,
    )
    .await?;
    if completed.is_none() {
        locked.tx.rollback().await?;
        return Ok(PreparationCompletionOutcome::NotCompletable);
    }

    let task = mark_task_prepared(
        &mut locked.tx,
        locked.task_id,
        locked.application_id,
        locked.environment_id,
        locked.job_id,
    )
    .await?;
    if task.is_none() {
        // The task transition and job result share this transaction. A changed
        // owner or a failed compare-and-set rolls the job update back too.
        locked.tx.rollback().await?;
        return Ok(PreparationCompletionOutcome::NotCompletable);
    }
    locked.tx.commit().await?;
    Ok(PreparationCompletionOutcome::Prepared)
}

/// Fail one claimed preparation atomically, retaining both exclusivity anchors.
pub(super) async fn fail(
    pool: &PgPool,
    manager_id: i64,
    job_id: Uuid,
) -> anyhow::Result<PreparationCompletionOutcome> {
    let mut locked = match lock_current(pool, manager_id, job_id).await? {
        LockAttempt::Locked(locked) => locked,
        LockAttempt::Refused(outcome) => return Ok(outcome),
    };

    let task = mark_preparation_uncertain(
        &mut locked.tx,
        locked.application_id,
        locked.environment_id,
        locked.task_id,
    )
    .await?;
    if task.is_none() {
        locked.tx.rollback().await?;
        return Ok(PreparationCompletionOutcome::NotCompletable);
    }
    let result = serde_json::to_value(ExecutionEnvironmentJobResult {
        outcome: "task_prepared_uncertain".to_string(),
        observed_state: None,
        confirmed: None,
        preparation_quiescent: None,
    })?;
    let completed = complete_job(
        &mut locked.tx,
        locked.job_id,
        locked.application_id,
        locked.environment_id,
        locked.task_id,
        manager_id,
        "uncertain",
        result,
        Some("assigned runner manager could not confirm the native task preparation"),
    )
    .await?;
    if completed.is_none() {
        locked.tx.rollback().await?;
        return Ok(PreparationCompletionOutcome::NotCompletable);
    }
    locked.tx.commit().await?;
    Ok(PreparationCompletionOutcome::Uncertain)
}

fn ack_matches(
    ack: &NativeTaskPreparedResult,
    task_id: Uuid,
    manifest_sha256: &str,
    file_count: u32,
    total_bytes: u32,
) -> bool {
    ack.prepared
        && ack.task_id == task_id
        && ack.manifest_sha256 == manifest_sha256
        && ack.file_count == file_count
        && ack.total_bytes == total_bytes
}
