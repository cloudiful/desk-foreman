use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::config::SharedRunnerManagerConfig;

pub struct SessionGate {
    active: Mutex<usize>,
    notify: Notify,
}

impl SessionGate {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            active: Mutex::new(0),
            notify: Notify::new(),
        })
    }

    pub async fn acquire(self: &Arc<Self>, config: &SharedRunnerManagerConfig) -> SessionPermit {
        loop {
            let notified = self.notify.notified();
            let max_sessions = config.read().await.max_sessions.max(1);
            let acquired = {
                let mut active = self.active.lock().expect("session gate lock poisoned");
                if *active < max_sessions {
                    *active += 1;
                    true
                } else {
                    false
                }
            };
            if acquired {
                return SessionPermit {
                    gate: Arc::clone(self),
                };
            }
            notified.await;
        }
    }
}

pub struct SessionPermit {
    gate: Arc<SessionGate>,
}

impl Drop for SessionPermit {
    fn drop(&mut self) {
        let mut active = self.gate.active.lock().expect("session gate lock poisoned");
        *active = active.saturating_sub(1);
        drop(active);
        self.gate.notify.notify_one();
    }
}
