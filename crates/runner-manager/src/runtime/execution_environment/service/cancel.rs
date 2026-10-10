//! Sticky per-environment cancellation for relayed streams (issue 749 P2c).
//!
//! One registry lives on the Docker backend beside the per-owner operation
//! locks. A relayed stream registers a watch receiver while it still holds
//! the environment mutex and file reservation, and releases both before it
//! touches the network; `stop` cancels the environment's receivers after it
//! persists the stopped generation, inside the same guard. The durable
//! barrier stays the authority on what may serve — this only wakes streams
//! that are already running so they stop waiting on a generation that is
//! now closed.
//!
//! Sticky means cancel-before-first-poll still fires: the receiver observes
//! the cancellation even when the stream has not polled it yet, because the
//! value is retained in the channel rather than delivered as a transient
//! event.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use tokio::sync::watch;

/// Wake-up registry for live relayed streams, keyed by environment id.
#[derive(Clone, Debug, Default)]
pub struct ServiceCancelRegistry {
    inner: Arc<Mutex<HashMap<String, Vec<watch::Sender<bool>>>>>,
}

impl ServiceCancelRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register one stream of `environment_id`.
    ///
    /// Call only while holding the environment mutex and file reservation,
    /// after the barrier admitted the generation: a stop that commits first
    /// persists its generation under the same locks, so it either cancels
    /// this registration or the barrier refuses it before it registers.
    pub fn register(&self, environment_id: &str) -> watch::Receiver<bool> {
        let (sender, receiver) = watch::channel(false);
        let mut inner = self.inner.lock().expect("service cancel map poisoned");
        let senders = inner.entry(environment_id.to_string()).or_default();
        senders.retain(|sender| !sender.is_closed());
        senders.push(sender);
        receiver
    }

    /// Cancel every live stream of one environment.
    ///
    /// Call only after persisting the stopped generation, inside the same
    /// guard. Returns how many streams were woken. A repeated cancel wakes
    /// nothing because the first one removed the entry.
    pub fn cancel_environment(&self, environment_id: &str) -> usize {
        let senders = self
            .inner
            .lock()
            .expect("service cancel map poisoned")
            .remove(environment_id);
        let Some(senders) = senders else {
            return 0;
        };
        let mut woken = 0usize;
        for sender in senders {
            if sender.send(true).is_ok() {
                woken += 1;
            }
        }
        woken
    }
}
