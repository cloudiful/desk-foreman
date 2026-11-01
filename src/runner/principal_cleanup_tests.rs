use super::super::{PendingJob, QueuedJob, RunnerBroker};
use runner_protocol::{RunnerJob, RunnerJobResult};
use serde_json::json;
use std::{
    collections::{BTreeSet, VecDeque},
    sync::Arc,
};
use tokio::sync::oneshot;

#[test]
fn complete_registered_roster_requires_every_live_manager_and_stable_membership() {
    use super::{changed_roster, coverage};
    assert!(coverage(&BTreeSet::new(), &BTreeSet::new(), &BTreeSet::new()).is_err());
    let roster = BTreeSet::from([1, 2, 3]);
    let live = BTreeSet::from([1, 3, 99]);
    let (targets, failures) = coverage(&roster, &live, &BTreeSet::from([1, 3])).unwrap();
    assert_eq!(targets, BTreeSet::from([1, 3]));
    assert_eq!(failures.len(), 1);
    let (targets, failures) = coverage(&roster, &live, &BTreeSet::from([1, 4])).unwrap();
    assert_eq!(targets, BTreeSet::from([1, 3]));
    assert_eq!(failures.len(), 2);
    assert!(failures.iter().any(|failure| failure.contains("manager 4")));
    assert!(coverage(&roster, &live, &BTreeSet::from([0])).is_err());
    assert!(failures[0].contains("manager 2"));
    assert!(changed_roster(&roster, &roster).is_none());
    assert!(
        changed_roster(&roster, &BTreeSet::from([1, 3, 4]))
            .unwrap()
            .contains("4")
    );
}

#[test]
fn manager_failures_and_malformed_responses_never_confirm() {
    use super::{aggregate_stop, cleanup_failure, stop_failure};
    use runner_protocol::{
        CleanupWorkspaceBindingPrincipalsAck, RunnerOwner, StopWorkspacePrincipalAck,
        StopWorkspacePrincipalState as State, WorkspacePrincipalCleanupResult,
    };
    use std::collections::BTreeMap;
    let success = StopWorkspacePrincipalAck {
        state: State::Stopped,
        detail: None,
    };
    assert!(stop_failure(1, &success).is_none());
    let failed = StopWorkspacePrincipalAck {
        state: State::Failed,
        detail: Some("cannot confirm".into()),
    };
    let aggregate = aggregate_stop(
        BTreeMap::from([(1, success.clone()), (2, failed)]),
        Vec::new(),
    );
    assert_eq!(aggregate.state, State::Failed);
    assert!(aggregate.detail.unwrap().contains("manager 2"));
    let aggregate = aggregate_stop(
        BTreeMap::from([(1, success.clone())]),
        vec!["manager 2: timeout".into()],
    );
    assert_eq!(aggregate.state, State::Failed);
    assert!(aggregate.detail.unwrap().contains("manager 2"));
    assert_eq!(
        aggregate_stop(BTreeMap::from([(1, success)]), Vec::new()).state,
        State::Stopped
    );
    for state in [
        State::Conflict,
        State::Busy,
        State::Failed,
        State::Unsupported,
    ] {
        assert!(
            stop_failure(
                2,
                &StopWorkspacePrincipalAck {
                    state,
                    detail: None
                }
            )
            .unwrap()
            .contains("manager 2")
        );
    }
    assert!(
        serde_json::from_value::<StopWorkspacePrincipalAck>(
            json!({"state":"stopped","detail":null,"unexpected":true})
        )
        .is_err()
    );
    let mut cleanup = CleanupWorkspaceBindingPrincipalsAck {
        state: State::Stopped,
        owners: Vec::new(),
        detail: None,
    };
    assert!(cleanup_failure(7, 1, &cleanup).is_none());
    cleanup.owners.push(WorkspacePrincipalCleanupResult {
        owner: RunnerOwner::WorkspaceBinding {
            workspace_binding_id: 1,
        },
        state: State::Stopped,
        detail: None,
    });
    assert!(
        cleanup_failure(7, 1, &cleanup)
            .unwrap()
            .contains("manager 7")
    );
}

fn broker() -> Arc<RunnerBroker> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .unwrap();
    RunnerBroker::new(pool)
}

#[tokio::test]
#[ignore = "requires newly initialized disposable P4k5 PostgreSQL"]
async fn deleted_manager_ledger_is_required_and_scoped() -> anyhow::Result<()> {
    use runner_protocol::{
        CleanupWorkspaceBindingPrincipalsRequest, StopWorkspacePrincipalRequest,
        StopWorkspacePrincipalState as State, workspace_principal::application_principal_owner,
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&std::env::var("P4K5_TEST_DATABASE_URL")?)
        .await?;
    sqlx::raw_sql(include_str!(
        "../sql/principal_admission/test_coverage_seed.sql"
    ))
    .execute(&pool)
    .await?;
    let owner = application_principal_owner(1, 701, "a", "w").unwrap();
    let other = application_principal_owner(1, 702, "b", "w").unwrap();
    let changing = application_principal_owner(1, 703, "c", "w").unwrap();
    for (id, subject, container) in [
        (817602, &owner, "deleted"),
        (817601, &owner, "live-a"),
        (817601, &owner, "live-b"),
        (817601, &other, "other"),
        (817601, &changing, "change"),
    ] {
        crate::db::principal_admission::admit(&pool, id, subject, container).await?;
    }
    sqlx::query(include_str!(
        "../sql/principal_admission/test_coverage_delete.sql"
    ))
    .execute(&pool)
    .await?;
    let broker = RunnerBroker::new(pool.clone());
    let worker = tokio::spawn({
        let broker = broker.clone();
        let pool = pool.clone();
        let changing = changing.clone();
        async move {
            for i in 0..4 {
                let job = tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    broker.next_job(817601),
                )
                .await
                .unwrap()
                .unwrap();
                if i == 3 {
                    crate::db::principal_admission::admit(&pool, 817602, &changing, "late")
                        .await
                        .unwrap();
                }
                let result = if job.kind == "stop_workspace_principal" {
                    json!({"state":"stopped", "detail":null})
                } else {
                    json!({"state":"stopped", "owners":[], "detail":null})
                };
                broker
                    .complete_job(
                        817601,
                        RunnerJobResult {
                            job_id: job.job_id,
                            ok: true,
                            result: Some(result),
                            error: None,
                        },
                    )
                    .await
                    .unwrap();
            }
        }
    });
    let make_request = |owner| StopWorkspacePrincipalRequest {
        owner,
        principal_id: "p".into(),
    };
    let ack = broker.stop_principal_all(make_request(owner)).await?;
    assert_eq!(ack.state, State::Failed);
    assert!(ack.detail.unwrap().contains("817602"));
    let ack = broker
        .cleanup_binding_principals_all(CleanupWorkspaceBindingPrincipalsRequest {
            workspace_binding_id: 701,
        })
        .await?;
    assert_eq!(ack.state, State::Failed);
    assert!(ack.detail.unwrap().contains("817602"));
    assert_eq!(
        broker.stop_principal_all(make_request(other)).await?.state,
        State::Stopped
    );
    let ack = broker.stop_principal_all(make_request(changing)).await?;
    assert_eq!(ack.state, State::Failed);
    assert!(ack.detail.unwrap().contains("changed"));
    worker.await?;
    assert!(broker.jobs.lock().await.is_empty());
    pool.close().await;
    Ok(())
}

#[tokio::test]
async fn dropped_queued_receivers_are_pruned_and_dispatched_jobs_are_retained() {
    let broker = broker();
    let (sender, receiver) = oneshot::channel();
    drop(receiver);
    let (sent, recv) = oneshot::channel();
    drop(recv);
    broker.pending.lock().await.insert(
        "abandoned".into(),
        PendingJob {
            manager_id: None,
            sender,
        },
    );
    broker.pending.lock().await.insert(
        "dispatched".into(),
        PendingJob {
            manager_id: Some(2),
            sender: sent,
        },
    );
    *broker.jobs.lock().await = VecDeque::from([QueuedJob {
        job: RunnerJob {
            job_id: "abandoned".into(),
            kind: "x".into(),
            payload: json!({}),
        },
        target_manager_id: Some(1),
    }]);
    broker.discard_abandoned_queued().await;
    assert!(broker.jobs.lock().await.is_empty());
    let pending = broker.pending.lock().await;
    assert!(!pending.contains_key("abandoned"));
    assert!(pending.contains_key("dispatched"));
}

#[tokio::test]
async fn timeout_aborts_waiter_then_prunes_targeted_queued_job() {
    let broker = broker();
    let (sender, receiver) = oneshot::channel::<anyhow::Result<serde_json::Value>>();
    broker.pending.lock().await.insert(
        "timeout".into(),
        PendingJob {
            manager_id: None,
            sender,
        },
    );
    broker.jobs.lock().await.push_back(QueuedJob {
        job: RunnerJob {
            job_id: "timeout".into(),
            kind: "stop_workspace_principal".into(),
            payload: json!({}),
        },
        target_manager_id: Some(4),
    });
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(async move { receiver.await });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(10), tasks.join_next())
            .await
            .is_err()
    );
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    broker.discard_abandoned_queued().await;
    assert!(broker.jobs.lock().await.is_empty());
    assert!(broker.pending.lock().await.is_empty());
}

#[tokio::test]
async fn another_manager_cannot_complete_a_targeted_dispatched_job() {
    let broker = broker();
    let (sender, receiver) = oneshot::channel();
    broker.pending.lock().await.insert(
        "job".into(),
        PendingJob {
            manager_id: Some(4),
            sender,
        },
    );
    let result = || RunnerJobResult {
        job_id: "job".into(),
        ok: true,
        result: Some(json!({"state":"stopped","detail":null})),
        error: None,
    };
    assert!(broker.complete_job(5, result()).await.is_err());
    broker.complete_job(4, result()).await.unwrap();
    assert_eq!(receiver.await.unwrap().unwrap()["state"], "stopped");
}
