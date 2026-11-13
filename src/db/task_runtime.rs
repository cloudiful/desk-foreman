//! Durable task runtime facade for the domain jobs.
//!
//! The durable task library owns its schema, its SQL and its state machine.
//! This module is the only place where the desk application touches it: it
//! opens one queue per existing environment-job kind, maps one domain job to
//! exactly one task through the stable key `environment-job:<job UUID>`, and
//! exposes only the two operations the domain needs: tracking a successful
//! claim and settling a terminal domain outcome.
//!
//! The facade never owns a transaction. Every operation borrows the
//! transaction of the caller, so the domain write and the task write commit
//! or roll back together. The facade never reads the domain through another
//! connection and never exposes library types to the callers.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use cloudiful_tasks::{
    Attempt, AttemptStatus, EnqueueRequest, Failure, Queue, RetryPolicy, Task, TaskId, TaskKind,
    TaskStatus, WorkerConfig,
};

use crate::db::execution_environment_jobs::EXECUTION_ENVIRONMENT_JOB_KINDS;

/// Namespace of every task of this application.
pub const TASK_NAMESPACE: &str = "desk-foreman";

/// Lease time to live of a claimed task, in seconds.
pub const TASK_LEASE_TTL_SECS: u64 = 360;

/// Prefix of the stable idempotency key of one domain job.
const TASK_KEY_PREFIX: &str = "environment-job:";

/// Disposition recorded when the domain reports a successful outcome.
pub const DISPOSITION_SUCCEEDED: &str = "domain_succeeded";

/// Disposition recorded when the domain reports a failed, uncertain, expired
/// or superseded outcome.
pub const DISPOSITION_FAILED: &str = "domain_failed";

/// The terminal outcome of one domain job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobDisposition {
    /// The domain operation succeeded.
    Succeeded,
    /// The domain operation ended without success.
    Failed,
}

impl JobDisposition {
    /// The disposition of a terminal domain state.
    ///
    /// Only a successful domain outcome is a success; every other terminal
    /// state (failed, uncertain, superseded, expired) is a non-success seal.
    pub fn for_state(state: &str) -> Self {
        if state == "succeeded" {
            Self::Succeeded
        } else {
            Self::Failed
        }
    }

    /// The fixed disposition code stored with the task failure.
    pub fn code(self) -> &'static str {
        match self {
            Self::Succeeded => DISPOSITION_SUCCEEDED,
            Self::Failed => DISPOSITION_FAILED,
        }
    }

    /// Whether a terminal task state is compatible with this disposition.
    ///
    /// A cancellation that won inside the library is a valid non-success seal,
    /// never a business success.
    fn accepts(self, status: TaskStatus) -> bool {
        match self {
            Self::Succeeded => status == TaskStatus::Succeeded,
            Self::Failed => matches!(status, TaskStatus::Failed | TaskStatus::Cancelled),
        }
    }
}

/// The stable idempotency key of one domain job.
pub fn task_key(job_id: Uuid) -> String {
    format!("{TASK_KEY_PREFIX}{job_id}")
}

/// The worker identity of one claimed task: the desk identity, the assigned
/// runner manager and the immutable job UUID.
pub fn worker_id(job_id: Uuid, manager_id: i64) -> String {
    format!("desk-foreman:manager:{manager_id}:job:{job_id}")
}

/// Whether the kind of a task is the kind of the scope it was found in.
fn kind_matches(kind: &str, expected_kind: &str) -> bool {
    kind == expected_kind
}

/// Whether the payload of a task is the payload of one domain job.
fn payload_matches(payload: &Value, job_id: Uuid) -> bool {
    *payload == json!({ "job_id": job_id })
}

/// The opened queues of the runtime, one per existing job kind.
struct Queues {
    by_kind: BTreeMap<String, Queue>,
}

impl Queues {
    fn queue(&self, job_kind: &str) -> anyhow::Result<&Queue> {
        self.by_kind.get(job_kind).ok_or_else(|| {
            anyhow::anyhow!("environment job kind `{job_kind}` has no durable task queue")
        })
    }

    /// Every task of the application that carries the exact key.
    ///
    /// The lookup is bounded to the five known queues of the namespace. Each
    /// match keeps the kind of the queue that returned it, so a task of another
    /// queue never passes for the task of the expected kind.
    async fn find_matches(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        key: &str,
    ) -> anyhow::Result<Vec<QueueMatch>> {
        let mut matches = Vec::new();
        for kind in EXECUTION_ENVIRONMENT_JOB_KINDS {
            let queue = self.queue(kind)?;
            let task = queue
                .get_by_idempotency_key_in(&mut *tx, key)
                .await
                .map_err(|error| anyhow::anyhow!("durable task lookup failed: {error}"))?;
            let Some(task) = task else {
                continue;
            };
            matches.push(QueueMatch {
                kind: (*kind).to_string(),
                task,
            });
        }
        Ok(matches)
    }
}

/// One task of the application together with the kind of the queue it was
/// found in.
struct QueueMatch {
    kind: String,
    task: Task,
}

/// The relation between the queue that owns a task and the kind the caller
/// expects for the job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchRelation {
    /// The task belongs to the expected queue, or the caller does not name a
    /// kind and the queue of the task is the authority.
    Compatible,
    /// The task belongs to another queue of the application.
    Foreign,
}

/// The relation between a found queue kind and the expected kind.
fn classify_match(found_kind: &str, expected_kind: Option<&str>) -> MatchRelation {
    match expected_kind {
        Some(expected_kind) if expected_kind != found_kind => MatchRelation::Foreign,
        _ => MatchRelation::Compatible,
    }
}

/// The identity of the task of one job.
///
/// A task of the application carries the kind of the queue that owns it and
/// the payload of exactly one job.
fn validate_task_identity(job_id: Uuid, queue_kind: &str, task: &Task) -> anyhow::Result<()> {
    if !kind_matches(task.kind().as_str(), queue_kind) {
        anyhow::bail!("the durable task of job {job_id} has a different kind");
    }
    if !payload_matches(task.payload(), job_id) {
        anyhow::bail!("the durable task of job {job_id} carries a different payload");
    }
    Ok(())
}

/// The durable task runtime of the application.
///
/// The runtime is required: an application state without an initialized runtime
/// refuses every claim and every settlement instead of running a job without a
/// durable trace.
pub struct TaskRuntime {
    pool: PgPool,
    queues: tokio::sync::OnceCell<Queues>,
}

impl TaskRuntime {
    /// Creates a runtime over the existing pool. No connection is checked out
    /// and no queue is opened here.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            queues: tokio::sync::OnceCell::new(),
        }
    }

    /// Migrates the library schema and opens the five queue handles.
    ///
    /// The call is idempotent: the handles are opened once and reused.
    pub async fn initialize(&self) -> anyhow::Result<()> {
        self.queues
            .get_or_try_init(|| async {
                let db = sea_orm::DatabaseConnection::from(self.pool.clone());
                cloudiful_tasks::migrate(&db)
                    .await
                    .map_err(|error| anyhow::anyhow!("durable task migration failed: {error}"))?;
                let config = WorkerConfig {
                    lease_ttl: Duration::from_secs(TASK_LEASE_TTL_SECS),
                    ..WorkerConfig::default()
                };
                let mut by_kind = BTreeMap::new();
                for kind in EXECUTION_ENVIRONMENT_JOB_KINDS {
                    let queue = Queue::open(db.clone(), TASK_NAMESPACE, kind, &config)
                        .await
                        .map_err(|error| {
                            anyhow::anyhow!("durable task queue `{kind}` is unavailable: {error}")
                        })?;
                    by_kind.insert(kind.to_string(), queue);
                }
                Ok::<Queues, anyhow::Error>(Queues { by_kind })
            })
            .await?;
        Ok(())
    }

    /// Whether the runtime is initialized.
    pub fn is_initialized(&self) -> bool {
        self.queues.initialized()
    }

    async fn queues(&self) -> anyhow::Result<&Queues> {
        self.queues
            .get()
            .ok_or_else(|| anyhow::anyhow!("the durable task runtime is not initialized"))
    }

    /// Tracks a successful domain claim.
    ///
    /// The task is enqueued and claimed by its exact identity inside the
    /// transaction of the caller. A claim that the library refuses rolls the
    /// domain claim back: a claimed domain job always owns a claimed task.
    pub async fn track_claim(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        job_id: Uuid,
        job_kind: &str,
        manager_id: i64,
    ) -> anyhow::Result<()> {
        let queues = self.queues().await?;
        let queue = queues.queue(job_kind)?;
        let key = task_key(job_id);
        let matches = queues.find_matches(&mut *tx, &key).await?;
        if matches.len() > 1 {
            anyhow::bail!("job {job_id} owns more than one durable task");
        }
        if let Some(found) = matches.first() {
            if classify_match(&found.kind, Some(job_kind)) == MatchRelation::Foreign {
                anyhow::bail!("the durable task of job {job_id} belongs to another queue");
            }
            validate_task_identity(job_id, &found.kind, &found.task)?;
        }
        let request = EnqueueRequest::new(
            TaskKind::new(job_kind)
                .map_err(|error| anyhow::anyhow!("invalid environment job kind: {error}"))?,
            json!({ "job_id": job_id }),
        )
        .with_idempotency_key(task_key(job_id))
        .map_err(|error| anyhow::anyhow!("invalid durable task key: {error}"))?
        .with_retry_policy(RetryPolicy::none());

        let outcome = queue
            .enqueue_in(&mut *tx, request)
            .await
            .map_err(|error| anyhow::anyhow!("durable task enqueue failed: {error}"))?;
        let task = outcome.into_task();
        if !kind_matches(task.kind().as_str(), job_kind) {
            anyhow::bail!("the durable task of job {job_id} has a different kind");
        }

        let claim = queue
            .claim_by_id_in(&mut *tx, task.id(), &worker_id(job_id, manager_id))
            .await
            .map_err(|error| anyhow::anyhow!("durable task claim failed: {error}"))?;
        if claim.is_none() {
            anyhow::bail!("the durable task of job {job_id} could not be claimed");
        }
        Ok(())
    }

    /// Settles the terminal domain outcome of one job.
    ///
    /// A key that does not exist is the legacy job that was claimed before the
    /// durable integration; it is a no-op. Every other deviation from the
    /// expected terminal state is an error that rolls the domain write back.
    pub async fn settle(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        job_id: Uuid,
        job_kind: Option<&str>,
        disposition: JobDisposition,
    ) -> anyhow::Result<()> {
        let queues = self.queues().await?;
        let key = task_key(job_id);
        let matches = queues.find_matches(&mut *tx, &key).await?;
        if matches.len() > 1 {
            anyhow::bail!("job {job_id} owns more than one durable task");
        }
        let Some(found) = matches.first() else {
            return Ok(());
        };
        if classify_match(&found.kind, job_kind) == MatchRelation::Foreign {
            anyhow::bail!("the durable task of job {job_id} belongs to another queue");
        }
        validate_task_identity(job_id, &found.kind, &found.task)?;

        let queue = queues.queue(&found.kind)?;
        let scope_kind = found.kind.clone();
        let task = &found.task;

        let status = task.status();
        if status.is_terminal() {
            if disposition.accepts(status) {
                return Ok(());
            }
            anyhow::bail!("the durable task of job {job_id} is terminal in an incompatible state");
        }
        if status != TaskStatus::Running {
            anyhow::bail!("the durable task of job {job_id} is not running");
        }

        let attempt = latest_attempt(queue, &mut *tx, task).await?;
        let settled = match disposition {
            JobDisposition::Succeeded => queue.complete_in(&mut *tx, &attempt, None).await,
            JobDisposition::Failed => {
                let failure = Failure::permanent(Some(json!({
                    "disposition": disposition.code(),
                })));
                queue.fail_in(&mut *tx, &attempt, &failure).await
            }
        };
        match settled {
            Ok(task) => {
                if !disposition.accepts(task.status()) {
                    anyhow::bail!(
                        "the durable task of job {job_id} sealed an incompatible terminal state"
                    );
                }
                Ok(())
            }
            Err(cloudiful_tasks::Error::LeaseLost { .. }) => {
                recover_and_verify(
                    queue,
                    &mut *tx,
                    &key,
                    job_id,
                    &scope_kind,
                    task.id(),
                    disposition,
                )
                .await
            }
            Err(error) => Err(anyhow::anyhow!("durable task settlement failed: {error}")),
        }
    }
}

/// The newest attempt of a running task.
async fn latest_attempt(
    queue: &Queue,
    tx: &mut Transaction<'_, Postgres>,
    task: &Task,
) -> anyhow::Result<Attempt> {
    let attempts = queue
        .attempts_in(tx, task.id(), 1)
        .await
        .map_err(|error| anyhow::anyhow!("durable task attempts are unavailable: {error}"))?;
    let attempt = attempts
        .first()
        .ok_or_else(|| anyhow::anyhow!("the durable task of a claimed job has no attempt"))?;
    if attempt.status() != AttemptStatus::Running {
        anyhow::bail!("the durable task of a claimed job has no running attempt");
    }
    Ok(attempt.clone())
}

/// Recovers an expired lease and verifies that a compatible terminal exists.
///
/// The recovery seals the expired attempt of the captured task only. A
/// recovery that seals nothing is not a success: the task must show a
/// terminal state that matches the domain disposition.
async fn recover_and_verify(
    queue: &Queue,
    tx: &mut Transaction<'_, Postgres>,
    key: &str,
    job_id: Uuid,
    scope_kind: &str,
    task_id: TaskId,
    disposition: JobDisposition,
) -> anyhow::Result<()> {
    queue
        .recover_expired_by_id_in(&mut *tx, task_id)
        .await
        .map_err(|error| anyhow::anyhow!("durable task recovery failed: {error}"))?;

    let task = queue
        .get_by_idempotency_key_in(tx, key)
        .await
        .map_err(|error| anyhow::anyhow!("durable task lookup failed: {error}"))?;
    let Some(task) = task else {
        anyhow::bail!("the durable task of job {job_id} disappeared");
    };
    if task.id() != task_id {
        anyhow::bail!("the durable task of job {job_id} was replaced by another task");
    }
    validate_task_identity(job_id, scope_kind, &task)?;
    if !task.status().is_terminal() {
        anyhow::bail!("the durable task of job {job_id} lost its lease without a terminal state");
    }
    if !disposition.accepts(task.status()) {
        anyhow::bail!("the durable task of job {job_id} sealed an incompatible terminal state");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_task_key_is_stable_and_names_the_job() {
        let job_id = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
        assert_eq!(
            task_key(job_id),
            "environment-job:11111111-2222-3333-4444-555555555555"
        );
    }

    #[test]
    fn a_cancelled_task_is_never_a_business_success() {
        assert!(JobDisposition::Failed.accepts(TaskStatus::Failed));
        assert!(JobDisposition::Failed.accepts(TaskStatus::Cancelled));
        assert!(!JobDisposition::Succeeded.accepts(TaskStatus::Cancelled));
        assert!(JobDisposition::Succeeded.accepts(TaskStatus::Succeeded));
        assert!(!JobDisposition::Failed.accepts(TaskStatus::Succeeded));
        assert!(!JobDisposition::Failed.accepts(TaskStatus::Running));
    }

    #[test]
    fn only_the_kind_of_the_scope_accepts_the_task() {
        assert!(kind_matches("export_native_task", "export_native_task"));
        assert!(!kind_matches("export_native_task", "ensure_environment"));
        assert!(!kind_matches("ensure_environment", "export_native_task"));
    }

    #[test]
    fn only_the_payload_of_the_job_accepts_the_task() {
        let job_id = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
        let other_job_id = Uuid::parse_str("99999999-2222-3333-4444-555555555555").unwrap();
        assert!(payload_matches(&json!({ "job_id": job_id }), job_id));
        assert!(!payload_matches(&json!({ "job_id": other_job_id }), job_id));
        assert!(!payload_matches(
            &json!({ "job_id": job_id, "extra": true }),
            job_id
        ));
    }

    #[test]
    fn a_task_of_another_queue_is_never_the_task_of_the_job() {
        assert_eq!(
            classify_match("ensure", Some("ensure")),
            MatchRelation::Compatible
        );
        assert_eq!(classify_match("ensure", None), MatchRelation::Compatible);
        assert_eq!(
            classify_match("ensure", Some("stop")),
            MatchRelation::Foreign
        );
        assert_eq!(
            classify_match("stop", Some("ensure")),
            MatchRelation::Foreign
        );
    }
}
