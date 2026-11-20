use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use runner_protocol::{RetainedSessionState, RetainedSessionStatus};
use uuid::Uuid;

mod orchestration;
pub use orchestration::{
    Descriptor, Output, OutputError, StartError, cancel, current, final_result, input, output,
    start, status,
};

const CAPACITY: usize = 256;
const TERMINAL_TTL: Duration = Duration::from_secs(900);

#[derive(Clone)]
struct Entry {
    principal: String,
    binding: i64,
    owner: runner_protocol::RunnerOwner,
    manager: i64,
    session: Option<u64>,
    state: ExecutionState,
    terminal_at: Option<Instant>,
    uncertain_terminal: Option<RetainedSessionStatus>,
    observed: Option<RetainedSessionStatus>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    started_at: Instant,
    op_gate: std::sync::Arc<tokio::sync::Mutex<()>>,
}

#[derive(Clone)]
enum ExecutionState {
    Starting,
    Running,
    Uncertain,
    Terminal(RetainedSessionStatus),
}

#[derive(Default)]
pub struct ExecutionRegistry {
    entries: Mutex<HashMap<String, Entry>>,
    pub(super) start_gate: tokio::sync::Mutex<()>,
}

impl ExecutionRegistry {
    fn reserve(
        &self,
        principal: &str,
        binding: i64,
        owner: runner_protocol::RunnerOwner,
        manager: i64,
    ) -> Result<String, &'static str> {
        let mut entries = self.entries.lock().map_err(|_| "registry unavailable")?;
        let now = Instant::now();
        entries.retain(|_, entry| {
            entry
                .terminal_at
                .is_none_or(|at| now.duration_since(at) < TERMINAL_TTL)
        });
        if entries.len() >= CAPACITY {
            return Err("execution capacity exhausted");
        }
        let id = Uuid::new_v4().to_string();
        entries.insert(
            id.clone(),
            Entry {
                principal: principal.into(),
                binding,
                owner,
                manager,
                session: None,
                state: ExecutionState::Starting,
                terminal_at: None,
                uncertain_terminal: None,
                observed: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
                started_at: Instant::now(),
                op_gate: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            },
        );
        Ok(id)
    }

    fn get(
        &self,
        id: &str,
        principal: &str,
        binding: i64,
        owner: &runner_protocol::RunnerOwner,
    ) -> Option<Entry> {
        let entries = self.entries.lock().ok()?;
        let entry = entries.get(id)?;
        if entry.principal != principal
            || entry.binding != binding
            || &entry.owner != owner
            || entry
                .terminal_at
                .is_some_and(|at| at.elapsed() >= TERMINAL_TTL)
        {
            return None;
        }
        Some(entry.clone())
    }

    fn remove_starting(&self, id: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            if entries
                .get(id)
                .is_some_and(|entry| matches!(entry.state, ExecutionState::Starting))
            {
                entries.remove(id);
            }
        }
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut Entry)) {
        if let Ok(mut entries) = self.entries.lock() {
            if let Some(entry) = entries.get_mut(id) {
                f(entry);
                entry.updated_at = chrono::Utc::now();
            }
        }
    }
}

impl Entry {
    fn mark_uncertain(&mut self, evidence: Option<RetainedSessionStatus>) {
        if matches!(self.state, ExecutionState::Terminal(_)) {
            return;
        }
        if let Some(evidence) = evidence {
            self.uncertain_terminal = Some(evidence);
        }
        self.state = ExecutionState::Uncertain;
        self.terminal_at.get_or_insert_with(Instant::now);
    }

    fn observe(&mut self, status: RetainedSessionStatus) {
        if let ExecutionState::Terminal(previous) = &mut self.state {
            if previous.state == status.state && previous.exit_code == status.exit_code {
                previous.output_complete |= status.output_complete;
                previous.stdout_bytes = previous.stdout_bytes.max(status.stdout_bytes);
                previous.stderr_bytes = previous.stderr_bytes.max(status.stderr_bytes);
                previous.dropped_bytes = previous.dropped_bytes.max(status.dropped_bytes);
                self.observed = Some(previous.clone());
                self.updated_at = chrono::Utc::now();
            }
            return;
        }
        if matches!(self.state, ExecutionState::Uncertain) {
            return;
        }
        self.observed = Some(status.clone());
        self.updated_at = chrono::Utc::now();
        if status.state == RetainedSessionState::Running {
            self.state = ExecutionState::Running;
        } else {
            self.state = ExecutionState::Terminal(status);
            self.terminal_at = Some(Instant::now());
        }
    }
}

#[cfg(test)]
mod tests;
