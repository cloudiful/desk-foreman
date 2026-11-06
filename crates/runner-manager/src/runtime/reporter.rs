use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use reqwest::Client;
use runner_protocol::RunnerLifecycleEvent;
use tokio::{
    sync::{mpsc, oneshot},
    time::{interval, sleep, timeout},
};

use crate::config::SharedRunnerManagerConfig;

struct Envelope {
    event: RunnerLifecycleEvent,
    ack: Option<oneshot::Sender<anyhow::Result<()>>>,
}

pub struct RunnerLifecycleReporter {
    sender: mpsc::Sender<Envelope>,
    critical_sender: mpsc::UnboundedSender<Envelope>,
    #[cfg(test)]
    noop: bool,
}

impl RunnerLifecycleReporter {
    pub fn spawn(config: SharedRunnerManagerConfig) -> Arc<Self> {
        let (sender, receiver) = mpsc::channel(256);
        let (critical_sender, critical_receiver) = mpsc::unbounded_channel();
        tokio::spawn(run_reporter(config, receiver, critical_receiver));
        Arc::new(Self {
            sender,
            critical_sender,
            #[cfg(test)]
            noop: false,
        })
    }

    #[cfg(test)]
    pub fn noop() -> Arc<Self> {
        let (sender, _receiver) = mpsc::channel(1);
        let (critical_sender, _critical_receiver) = mpsc::unbounded_channel();
        Arc::new(Self {
            sender,
            critical_sender,
            noop: true,
        })
    }

    pub fn report(&self, event: RunnerLifecycleEvent) {
        self.enqueue(Envelope { event, ack: None });
    }

    /// Report one lifecycle event and wait until the control plane confirms it
    /// with an HTTP 2xx. Enqueue and acknowledgement share one absolute budget.
    pub async fn report_and_wait(&self, event: RunnerLifecycleEvent) -> anyhow::Result<()> {
        #[cfg(test)]
        if self.noop {
            return Ok(());
        }
        let (ack, receipt) = oneshot::channel();
        let envelope = Envelope {
            event,
            ack: Some(ack),
        };
        let wait = async {
            self.sender
                .send(envelope)
                .await
                .map_err(|_| worker_unavailable())?;
            receipt.await.map_err(|_| report_failed())?
        };
        timeout(Duration::from_secs(15), wait)
            .await
            .map_err(|_| acknowledgement_timed_out())?
    }

    fn enqueue(&self, envelope: Envelope) {
        match self.sender.try_send(envelope) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(envelope)) => {
                if envelope.event.status != runner_protocol::RunnerLifecycleStatus::Running {
                    if let Err(error) = self.critical_sender.send(envelope) {
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

fn worker_unavailable() -> anyhow::Error {
    anyhow::anyhow!("runner lifecycle report worker unavailable")
}

fn acknowledgement_timed_out() -> anyhow::Error {
    anyhow::anyhow!("runner lifecycle report acknowledgement timed out")
}

fn report_failed() -> anyhow::Error {
    anyhow::anyhow!("runner lifecycle report failed before acknowledgement")
}

fn report_superseded() -> anyhow::Error {
    anyhow::anyhow!("runner lifecycle report superseded")
}

async fn run_reporter(
    config: SharedRunnerManagerConfig,
    mut receiver: mpsc::Receiver<Envelope>,
    mut critical_receiver: mpsc::UnboundedReceiver<Envelope>,
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
    let mut pending = HashMap::<String, Envelope>::new();
    let mut known = HashMap::<String, RunnerLifecycleEvent>::new();
    let mut last_sent = HashMap::<String, Instant>::new();
    let mut flush = interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            Some(envelope) = critical_receiver.recv() => {
                let key = envelope.event.owner.stable_key();
                known.remove(&key);
                last_sent.remove(&key);
                queue_event(&mut pending, envelope);
            }
            Some(envelope) = receiver.recv() => {
                let key = envelope.event.owner.stable_key();
                if envelope.event.status == runner_protocol::RunnerLifecycleStatus::Running {
                    known.insert(key.clone(), envelope.event.clone());
                } else {
                    known.remove(&key);
                    last_sent.remove(&key);
                }
                queue_event(&mut pending, envelope);
            }
            _ = flush.tick() => {
                enqueue_due_heartbeats(&mut pending, &known, &last_sent, Instant::now());
                if pending.is_empty() {
                    continue;
                }
                match flush_reports(&client, &config, &mut pending).await {
                    Ok(sent) => {
                        for envelope in sent {
                            let event = envelope.event;
                            if event.status == runner_protocol::RunnerLifecycleStatus::Running {
                                last_sent.insert(event.owner.stable_key(), Instant::now());
                            }
                            if let Some(ack) = envelope.ack {
                                let _ = ack.send(Ok(()));
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "failed to report runner lifecycle state");
                        fail_acked_pending(&mut pending);
                        sleep(Duration::from_secs(5)).await;
                    }
                }
            }
            else => return,
        }
    }
}

fn queue_event(pending: &mut HashMap<String, Envelope>, envelope: Envelope) {
    let key = envelope.event.owner.stable_key();
    let replace = pending.get(&key).is_none_or(|existing| {
        existing.event.status == runner_protocol::RunnerLifecycleStatus::Running
            || envelope.event.status != runner_protocol::RunnerLifecycleStatus::Running
    });
    if replace {
        if let Some(previous) = pending.insert(key, envelope) {
            if let Some(ack) = previous.ack {
                let _ = ack.send(Err(report_superseded()));
            }
        }
    } else if let Some(ack) = envelope.ack {
        let _ = ack.send(Err(report_superseded()));
    }
}

fn enqueue_due_heartbeats(
    pending: &mut HashMap<String, Envelope>,
    known: &HashMap<String, RunnerLifecycleEvent>,
    last_sent: &HashMap<String, Instant>,
    now: Instant,
) {
    for (key, event) in known {
        if pending.contains_key(key) {
            continue;
        }
        if last_sent
            .get(key)
            .is_none_or(|last| now.duration_since(*last) >= Duration::from_secs(15))
        {
            queue_event(
                pending,
                Envelope {
                    event: event.clone(),
                    ack: None,
                },
            );
        }
    }
}

fn fail_acked_pending(pending: &mut HashMap<String, Envelope>) {
    let acked = pending
        .iter()
        .filter(|(_, envelope)| envelope.ack.is_some())
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    for key in acked {
        if let Some(envelope) = pending.remove(&key) {
            if let Some(ack) = envelope.ack {
                let _ = ack.send(Err(report_failed()));
            }
        }
    }
}

async fn flush_reports(
    client: &Client,
    config: &SharedRunnerManagerConfig,
    pending: &mut HashMap<String, Envelope>,
) -> anyhow::Result<Vec<Envelope>> {
    let (base_url, auth_token) = {
        let config = config.read().await;
        (config.control_plane_url.clone(), config.auth_token.clone())
    };
    let Some(base_url) = base_url else {
        for (_, envelope) in pending.drain() {
            if let Some(ack) = envelope.ack {
                let _ = ack.send(Err(report_failed()));
            }
        }
        return Ok(Vec::new());
    };
    let entries = pending
        .iter()
        .map(|(key, envelope)| (key.clone(), envelope.event.clone()))
        .collect::<Vec<_>>();
    let payload = entries.iter().map(|(_, event)| event).collect::<Vec<_>>();
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
    let mut sent = Vec::new();
    for (key, event) in entries {
        if pending
            .get(&key)
            .is_some_and(|current| current.event == event)
        {
            if let Some(envelope) = pending.remove(&key) {
                sent.push(envelope);
            }
        }
    }
    Ok(sent)
}

#[cfg(test)]
#[path = "reporter_tests.rs"]
mod tests;
