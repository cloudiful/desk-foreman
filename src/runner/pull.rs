use std::{collections::VecDeque, sync::Arc};

use anyhow::Context;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::{Mutex, Notify, oneshot};
use tokio::time::{Duration, interval, timeout};
use uuid::Uuid;

use runner_protocol::{
    CancelSessionRequest, CommandOutput, ExecRequest, InputRequest, RUNNER_JOB_TIMEOUT_SECS,
    RunnerCommandRequest, RunnerJob, RunnerJobResult, RunnerSessionStatus, ShellToolOutput,
};

use crate::{
    db,
    runner::{RunnerFuture, RunnerService},
};

struct QueuedJob {
    job: RunnerJob,
}

struct PendingJob {
    manager_id: Option<i64>,
    sender: oneshot::Sender<anyhow::Result<Value>>,
}

pub struct RunnerBroker {
    db: sqlx::PgPool,
    jobs: Mutex<VecDeque<QueuedJob>>,
    pending: Mutex<std::collections::HashMap<String, PendingJob>>,
    notify: Notify,
}

impl RunnerBroker {
    pub fn new(db: sqlx::PgPool) -> Arc<Self> {
        Arc::new(Self {
            db,
            jobs: Mutex::new(VecDeque::new()),
            pending: Mutex::new(std::collections::HashMap::new()),
            notify: Notify::new(),
        })
    }

    pub fn spawn_liveness_monitor(self: &Arc<Self>) {
        let broker = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(10));
            loop {
                tick.tick().await;
                if let Err(error) = broker.expire_stale_jobs().await {
                    tracing::warn!(error = %error, "failed to expire stale runner jobs");
                }
            }
        });
    }

    async fn expire_stale_jobs(&self) -> anyhow::Result<()> {
        let live_manager_ids = db::queries::list_live_runner_manager_ids(&self.db).await?;
        let mut pending = self.pending.lock().await;
        let stale_ids = pending
            .iter()
            .filter(|(_, job)| {
                job.manager_id
                    .is_some_and(|id| !live_manager_ids.contains(&id))
            })
            .map(|(job_id, _)| job_id.clone())
            .collect::<Vec<_>>();
        for job_id in stale_ids {
            if let Some(job) = pending.remove(&job_id) {
                let _ = job
                    .sender
                    .send(Err(anyhow::anyhow!("runner manager disconnected")));
            }
        }
        Ok(())
    }

    async fn submit<T: serde::Serialize, R: DeserializeOwned>(
        &self,
        kind: &str,
        payload: &T,
    ) -> anyhow::Result<R> {
        db::queries::find_enabled_runner_manager(&self.db)
            .await?
            .context("no enabled runner manager is configured")?;
        let job_id = Uuid::new_v4().to_string();
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(
            job_id.clone(),
            PendingJob {
                manager_id: None,
                sender,
            },
        );
        self.jobs.lock().await.push_back(QueuedJob {
            job: RunnerJob {
                job_id: job_id.clone(),
                kind: kind.to_string(),
                payload: serde_json::to_value(payload)?,
            },
        });
        self.notify.notify_waiters();
        let value = match timeout(Duration::from_secs(RUNNER_JOB_TIMEOUT_SECS), receiver).await {
            Ok(result) => {
                result.context("runner manager disconnected before returning job result")??
            }
            Err(_) => {
                self.pending.lock().await.remove(&job_id);
                anyhow::bail!("runner job timed out waiting for manager result")
            }
        };
        Ok(serde_json::from_value(value)?)
    }

    pub async fn next_job(&self, manager_id: i64) -> Option<RunnerJob> {
        loop {
            let notified = self.notify.notified();
            let live_manager_ids = match db::queries::list_live_runner_manager_ids(&self.db).await {
                Ok(ids) => Some(ids),
                Err(error) => {
                    tracing::warn!(error = %error, "failed to check runner manager liveness");
                    None
                }
            };
            let job = {
                let mut pending = self.pending.lock().await;
                if let Some(live_manager_ids) = &live_manager_ids {
                    let stale_ids = pending
                        .iter()
                        .filter(|(_, job)| {
                            job.manager_id
                                .is_some_and(|id| !live_manager_ids.contains(&id))
                        })
                        .map(|(job_id, _)| job_id.clone())
                        .collect::<Vec<_>>();
                    for job_id in stale_ids {
                        if let Some(job) = pending.remove(&job_id) {
                            let _ = job
                                .sender
                                .send(Err(anyhow::anyhow!("runner manager disconnected")));
                        }
                    }
                }
                let mut jobs = self.jobs.lock().await;
                jobs.retain(|queued| pending.contains_key(&queued.job.job_id));
                let index = jobs.iter().position(|queued| {
                    pending
                        .get(&queued.job.job_id)
                        .is_some_and(|job| job.manager_id.is_none())
                });
                index.and_then(|index| {
                    let job = jobs.remove(index)?.job;
                    pending.get_mut(&job.job_id)?.manager_id = Some(manager_id);
                    Some(job)
                })
            };
            if let Some(job) = job {
                return Some(job);
            }
            notified.await;
        }
    }

    pub async fn complete_job(
        &self,
        manager_id: i64,
        result: RunnerJobResult,
    ) -> anyhow::Result<()> {
        let mut pending_jobs = self.pending.lock().await;
        let pending = pending_jobs
            .get(&result.job_id)
            .context("unknown runner job")?;
        if pending.manager_id != Some(manager_id) {
            anyhow::bail!("runner job belongs to another manager");
        }
        let pending = pending_jobs
            .remove(&result.job_id)
            .expect("pending runner job checked above");
        let value = match (result.ok, result.result, result.error) {
            (true, Some(value), _) => Ok(value),
            (false, _, Some(error)) => Err(anyhow::anyhow!(error)),
            _ => Err(anyhow::anyhow!(
                "runner manager returned an invalid job result"
            )),
        };
        let _ = pending.sender.send(value);
        Ok(())
    }
}

pub struct PullRunnerService {
    broker: Arc<RunnerBroker>,
}

impl PullRunnerService {
    pub fn new(broker: Arc<RunnerBroker>) -> Arc<Self> {
        Arc::new(Self { broker })
    }
}

impl RunnerService for PullRunnerService {
    fn exec_shell<'a>(
        &'a self,
        request: ExecRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async move { self.broker.submit("exec_shell", &request).await })
    }

    fn write_stdin<'a>(
        &'a self,
        request: InputRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async move { self.broker.submit("write_stdin", &request).await })
    }

    fn cancel_session<'a>(
        &'a self,
        request: CancelSessionRequest,
    ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>> {
        Box::pin(async move { self.broker.submit("cancel_session", &request).await })
    }

    fn list_sessions<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>> {
        Box::pin(async move { self.broker.submit("list_sessions", &()).await })
    }

    fn run_command<'a>(
        &'a self,
        request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
        Box::pin(async move { self.broker.submit("run_command", &request).await })
    }
}
