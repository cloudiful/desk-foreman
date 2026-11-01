use super::RunnerBroker;
use runner_protocol::{
    CleanupWorkspaceBindingPrincipalsAck, CleanupWorkspaceBindingPrincipalsRequest, RunnerOwner,
    StopWorkspacePrincipalAck, StopWorkspacePrincipalRequest, StopWorkspacePrincipalState as State,
};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use tokio::{
    task::JoinSet,
    time::{Duration, Instant, timeout_at},
};

#[cfg(test)]
#[path = "principal_cleanup_tests.rs"]
mod tests;

fn coverage(
    roster: &BTreeSet<i64>,
    live: &BTreeSet<i64>,
    ledger: &BTreeSet<i64>,
) -> anyhow::Result<(BTreeSet<i64>, Vec<String>)> {
    if roster.is_empty() {
        anyhow::bail!("no registered runner managers");
    }
    let pending = roster.intersection(live).copied().collect();
    if ledger.iter().any(|id| *id <= 0) {
        anyhow::bail!("invalid admission manager id");
    }
    let mut failures: Vec<String> = roster
        .difference(live)
        .map(|id| {
            format!("manager {id}: offline or disabled; confirmation required even if never used")
        })
        .collect();
    failures.extend(
        ledger
            .difference(roster)
            .map(|id| format!("manager {id}: admission manager no longer registered")),
    );
    Ok((pending, failures))
}

fn changed_roster(before: &BTreeSet<i64>, after: &BTreeSet<i64>) -> Option<String> {
    (before != after).then(|| format!("manager roster changed: before {before:?}, after {after:?}"))
}

fn stop_failure(id: i64, ack: &StopWorkspacePrincipalAck) -> Option<String> {
    (ack.state != State::Stopped)
        .then(|| format!("manager {id}: {:?}: {:?}", ack.state, ack.detail))
}

fn aggregate_stop(
    results: BTreeMap<i64, StopWorkspacePrincipalAck>,
    mut failures: Vec<String>,
) -> StopWorkspacePrincipalAck {
    for (id, ack) in results {
        if let Some(failure) = stop_failure(id, &ack) {
            failures.push(failure);
        }
    }
    StopWorkspacePrincipalAck {
        state: if failures.is_empty() {
            State::Stopped
        } else {
            State::Failed
        },
        detail: if failures.is_empty() {
            None
        } else {
            Some(failures.join("; "))
        },
    }
}

fn cleanup_failure(
    id: i64,
    binding_id: i64,
    ack: &CleanupWorkspaceBindingPrincipalsAck,
) -> Option<String> {
    let bad_owners = ack.owners.iter().any(|entry| entry.state != State::Stopped || !matches!(&entry.owner, RunnerOwner::ApplicationPrincipal { workspace_binding_id, .. } if *workspace_binding_id == binding_id) || !(StopWorkspacePrincipalRequest { owner: entry.owner.clone(), principal_id: "validated".into() }).is_valid());
    (ack.state != State::Stopped || bad_owners).then(|| {
        format!(
            "manager {id}: {:?}: {:?}; invalid or failed owner results: {bad_owners}",
            ack.state, ack.detail
        )
    })
}

enum CoverageScope {
    Owner(RunnerOwner),
    Binding(i64),
}

impl CoverageScope {
    async fn manager_ids(&self, pool: &sqlx::PgPool) -> anyhow::Result<BTreeSet<i64>> {
        let rows = match self {
            Self::Owner(owner) => {
                crate::db::principal_admission::list_active_by_owner(pool, owner).await?
            }
            Self::Binding(id) => {
                crate::db::principal_admission::list_active_by_binding(pool, *id).await?
            }
        };
        let ids: BTreeSet<i64> = rows.into_iter().map(|row| row.runner_manager_id).collect();
        if ids.iter().any(|id| *id <= 0) {
            anyhow::bail!("invalid admission manager id");
        }
        Ok(ids)
    }
}

impl RunnerBroker {
    async fn discard_abandoned_queued(&self) {
        let mut pending = self.pending.lock().await;
        pending.retain(|_, job| !job.sender.is_closed() || job.manager_id.is_some());
        let mut jobs = self.jobs.lock().await;
        jobs.retain(|job| pending.contains_key(&job.job.job_id));
    }

    async fn covered_jobs<T, R>(
        self: &Arc<Self>,
        kind: &'static str,
        payload: T,
        scope: CoverageScope,
    ) -> anyhow::Result<(BTreeMap<i64, R>, Vec<String>)>
    where
        T: Serialize + Clone + Send + Sync + 'static,
        R: DeserializeOwned + Send + 'static,
    {
        let deadline = Instant::now() + Duration::from_secs(20);
        let roster = timeout_at(
            deadline,
            crate::db::runner_managers::list_registered_runner_manager_ids(&self.db),
        )
        .await
        .map_err(|_| anyhow::anyhow!("manager registration lookup timed out"))??;
        let roster: BTreeSet<i64> = roster.into_iter().collect();
        if roster.is_empty() {
            anyhow::bail!("no registered runner managers");
        }
        let live: BTreeSet<i64> = timeout_at(
            deadline,
            crate::db::runner_managers::list_live_runner_manager_ids(&self.db),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!("manager liveness lookup timed out for registered managers {roster:?}")
        })?
        .map_err(|e| {
            anyhow::anyhow!(
                "manager liveness lookup failed for registered managers {roster:?}: {e}"
            )
        })?
        .into_iter()
        .collect();
        let ledger = timeout_at(deadline, scope.manager_ids(&self.db))
            .await
            .map_err(|_| anyhow::anyhow!("admission manager lookup timed out"))?
            .map_err(|_| anyhow::anyhow!("admission manager lookup failed"))?;
        let (mut pending, mut failures) = coverage(&roster, &live, &ledger)?;
        let mut jobs = JoinSet::new();
        for &id in &pending {
            let broker = Arc::clone(self);
            let payload = payload.clone();
            jobs.spawn(async move {
                (
                    id,
                    broker.submit_to_manager::<T, R>(id, kind, &payload).await,
                )
            });
        }
        let mut results = BTreeMap::new();
        while !pending.is_empty() {
            match timeout_at(deadline, jobs.join_next()).await {
                Ok(Some(Ok((id, result)))) => {
                    pending.remove(&id);
                    match result {
                        Ok(value) => {
                            results.insert(id, value);
                        }
                        Err(error) => failures.push(format!("manager {id}: {error}")),
                    }
                }
                Ok(Some(Err(error))) => {
                    failures.push(format!(
                        "manager task failed: {error}; unresolved {pending:?}"
                    ));
                    break;
                }
                Ok(None) => {
                    failures.push(format!("missing manager responses: {pending:?}"));
                    break;
                }
                Err(_) => {
                    failures.push(format!("manager responses timed out: {pending:?}"));
                    break;
                }
            }
        }
        jobs.abort_all();
        while jobs.join_next().await.is_some() {}
        self.discard_abandoned_queued().await;
        match timeout_at(
            deadline,
            crate::db::runner_managers::list_registered_runner_manager_ids(&self.db),
        )
        .await
        {
            Ok(Ok(current)) => {
                let current: BTreeSet<i64> = current.into_iter().collect();
                if let Some(error) = changed_roster(&roster, &current) {
                    failures.push(error);
                }
            }
            Ok(Err(error)) => failures.push(format!(
                "manager roster recheck failed: {error}; registered {roster:?}"
            )),
            Err(_) => failures.push(format!("manager roster recheck timed out: {roster:?}")),
        }
        match timeout_at(deadline, scope.manager_ids(&self.db)).await {
            Ok(Ok(current)) if current == ledger => {}
            Ok(Ok(current)) => failures.push(format!(
                "admission manager set changed: before {ledger:?}, after {current:?}"
            )),
            Ok(Err(_)) => failures.push("admission manager recheck failed".into()),
            Err(_) => failures.push("admission manager recheck timed out".into()),
        }
        Ok((results, failures))
    }

    pub(super) async fn stop_principal_all(
        self: &Arc<Self>,
        request: StopWorkspacePrincipalRequest,
    ) -> anyhow::Result<StopWorkspacePrincipalAck> {
        if !request.is_valid() {
            anyhow::bail!("invalid principal stop request");
        }
        let scope = CoverageScope::Owner(request.owner.clone());
        let (results, failures) = self
            .covered_jobs::<_, StopWorkspacePrincipalAck>(
                "stop_workspace_principal",
                request,
                scope,
            )
            .await?;
        Ok(aggregate_stop(results, failures))
    }

    pub(super) async fn cleanup_binding_principals_all(
        self: &Arc<Self>,
        request: CleanupWorkspaceBindingPrincipalsRequest,
    ) -> anyhow::Result<CleanupWorkspaceBindingPrincipalsAck> {
        if request.workspace_binding_id <= 0 {
            anyhow::bail!("invalid workspace binding id");
        }
        let binding_id = request.workspace_binding_id;
        let (results, mut failures) = self
            .covered_jobs::<_, CleanupWorkspaceBindingPrincipalsAck>(
                "cleanup_workspace_binding_principals",
                request,
                CoverageScope::Binding(binding_id),
            )
            .await?;
        let mut owners = Vec::new();
        for (id, ack) in results {
            if let Some(failure) = cleanup_failure(id, binding_id, &ack) {
                failures.push(failure);
            }
            owners.extend(ack.owners);
        }
        Ok(CleanupWorkspaceBindingPrincipalsAck {
            state: if failures.is_empty() {
                State::Stopped
            } else {
                State::Failed
            },
            owners,
            detail: if failures.is_empty() {
                None
            } else {
                Some(failures.join("; "))
            },
        })
    }
}
