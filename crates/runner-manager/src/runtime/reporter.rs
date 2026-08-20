use std::{collections::HashMap, sync::Arc, time::Duration};

use reqwest::Client;
use runner_protocol::RunnerLifecycleEvent;
use tokio::{
    sync::mpsc,
    time::{interval, sleep},
};

use crate::config::SharedRunnerManagerConfig;

pub struct RunnerLifecycleReporter {
    sender: mpsc::Sender<RunnerLifecycleEvent>,
    critical_sender: mpsc::UnboundedSender<RunnerLifecycleEvent>,
}

impl RunnerLifecycleReporter {
    pub fn spawn(config: SharedRunnerManagerConfig) -> Arc<Self> {
        let (sender, receiver) = mpsc::channel(256);
        let (critical_sender, critical_receiver) = mpsc::unbounded_channel();
        tokio::spawn(run_reporter(config, receiver, critical_receiver));
        Arc::new(Self {
            sender,
            critical_sender,
        })
    }

    #[cfg(test)]
    pub fn noop() -> Arc<Self> {
        let (sender, _receiver) = mpsc::channel(1);
        let (critical_sender, _critical_receiver) = mpsc::unbounded_channel();
        Arc::new(Self {
            sender,
            critical_sender,
        })
    }

    pub fn report(&self, event: RunnerLifecycleEvent) {
        match self.sender.try_send(event) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(event)) => {
                if event.status != runner_protocol::RunnerLifecycleStatus::Running {
                    if let Err(error) = self.critical_sender.send(event) {
                        tracing::warn!(error = %error, "critical runner lifecycle report worker exited");
                    }
                } else {
                    tracing::warn!("runner lifecycle report queue is full");
                }
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                tracing::debug!("runner lifecycle report worker exited");
            }
        }
    }
}

async fn run_reporter(
    config: SharedRunnerManagerConfig,
    mut receiver: mpsc::Receiver<RunnerLifecycleEvent>,
    mut critical_receiver: mpsc::UnboundedReceiver<RunnerLifecycleEvent>,
) {
    let client = match Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            tracing::error!(%error, "failed to build runner lifecycle reporter client");
            return;
        }
    };
    let mut pending = HashMap::<String, RunnerLifecycleEvent>::new();
    let mut known = HashMap::<String, RunnerLifecycleEvent>::new();
    let mut last_sent = HashMap::<String, std::time::Instant>::new();
    let mut flush = interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            Some(event) = critical_receiver.recv() => {
                let key = event.owner.stable_key();
                known.remove(&key);
                last_sent.remove(&key);
                queue_event(&mut pending, event);
            }
            Some(event) = receiver.recv() => {
                let key = event.owner.stable_key();
                if event.status == runner_protocol::RunnerLifecycleStatus::Running {
                    known.insert(key.clone(), event.clone());
                } else {
                    known.remove(&key);
                    last_sent.remove(&key);
                }
                queue_event(&mut pending, event);
            }
            _ = flush.tick() => {
                for (key, event) in &known {
                    if last_sent
                        .get(key)
                        .is_none_or(|last| last.elapsed() >= Duration::from_secs(15))
                    {
                        queue_event(&mut pending, event.clone());
                    }
                }
                if pending.is_empty() {
                    continue;
                }
                match flush_reports(&client, &config, &mut pending).await {
                    Ok(sent) => {
                        for event in sent {
                            if event.status == runner_protocol::RunnerLifecycleStatus::Running {
                                last_sent.insert(event.owner.stable_key(), std::time::Instant::now());
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "failed to report runner lifecycle state");
                        sleep(Duration::from_secs(5)).await;
                    }
                }
            }
            else => return,
        }
    }
}

fn queue_event(pending: &mut HashMap<String, RunnerLifecycleEvent>, event: RunnerLifecycleEvent) {
    let key = event.owner.stable_key();
    let replace = pending.get(&key).is_none_or(|existing| {
        existing.status == runner_protocol::RunnerLifecycleStatus::Running
            || event.status != runner_protocol::RunnerLifecycleStatus::Running
    });
    if replace {
        pending.insert(key, event);
    }
}

async fn flush_reports(
    client: &Client,
    config: &SharedRunnerManagerConfig,
    pending: &mut HashMap<String, RunnerLifecycleEvent>,
) -> anyhow::Result<Vec<RunnerLifecycleEvent>> {
    let (base_url, auth_token) = {
        let config = config.read().await;
        (config.control_plane_url.clone(), config.auth_token.clone())
    };
    let Some(base_url) = base_url else {
        pending.clear();
        return Ok(Vec::new());
    };
    let events = pending
        .iter()
        .map(|(key, event)| (key.clone(), event.clone()))
        .collect::<Vec<_>>();
    let sent = events
        .iter()
        .map(|(_, event)| event.clone())
        .collect::<Vec<_>>();
    let payload = events.iter().map(|(_, event)| event).collect::<Vec<_>>();
    client
        .post(format!(
            "{}/api/internal/runner-manager/workspace-runners/report",
            base_url.trim_end_matches('/')
        ))
        .bearer_auth(auth_token)
        .json(&payload)
        .send()
        .await?
        .error_for_status()?;
    for (key, event) in events {
        if pending.get(&key).is_some_and(|current| current == &event) {
            pending.remove(&key);
        }
    }
    Ok(sent)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use runner_protocol::{RunnerLifecycleEvent, RunnerLifecycleStatus, RunnerOwner};

    use super::queue_event;

    fn event(status: RunnerLifecycleStatus) -> RunnerLifecycleEvent {
        RunnerLifecycleEvent {
            owner: RunnerOwner::InternalUser { user_id: 1 },
            container_name: "desk-foreman-runner-user-1".to_string(),
            container_id: None,
            status,
            workspace_root: None,
            runtime: None,
            runtime_class: None,
            image_name: None,
            network_enabled: None,
            last_error: None,
        }
    }

    #[test]
    fn removal_event_is_not_overwritten_by_later_activity() {
        let mut pending = HashMap::new();
        queue_event(&mut pending, event(RunnerLifecycleStatus::Removed));
        queue_event(&mut pending, event(RunnerLifecycleStatus::Running));
        assert_eq!(
            pending
                .get("user:1")
                .expect("event should remain queued")
                .status,
            RunnerLifecycleStatus::Removed
        );
    }

    #[test]
    fn queued_event_is_retained_when_newer_event_replaces_it() {
        let mut pending = HashMap::new();
        let running = event(RunnerLifecycleStatus::Running);
        queue_event(&mut pending, running);
        let removed = event(RunnerLifecycleStatus::Removed);
        queue_event(&mut pending, removed.clone());
        assert_eq!(pending.get("user:1").expect("event should exist"), &removed);
    }
}
