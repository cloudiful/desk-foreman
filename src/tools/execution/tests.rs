use super::*;

fn owner() -> runner_protocol::RunnerOwner {
    runner_protocol::RunnerOwner::InternalUser { user_id: 1 }
}

fn terminal() -> RetainedSessionStatus {
    RetainedSessionStatus {
        state: RetainedSessionState::Exited,
        exit_code: Some(7),
        output_complete: false,
        stdout_bytes: 0,
        stderr_bytes: 0,
        dropped_bytes: 0,
    }
}

#[test]
fn execution_registry_owner_capacity_and_expiry() {
    let registry = ExecutionRegistry::default();
    let first = registry.reserve("p", 1, owner(), 2).unwrap();
    assert!(registry.get(&first, "q", 1, &owner()).is_none());
    assert!(registry.get(&first, "p", 2, &owner()).is_none());
    for _ in 1..CAPACITY {
        registry.reserve("p", 1, owner(), 2).unwrap();
    }
    assert!(registry.reserve("p", 1, owner(), 2).is_err());
    registry.update(&first, |entry| {
        entry.mark_uncertain(None);
        entry.terminal_at = Some(Instant::now() - TERMINAL_TTL);
    });
    assert!(registry.reserve("p", 1, owner(), 2).is_ok());
}

#[test]
fn execution_registry_terminal_and_uncertain_are_monotonic() {
    let registry = ExecutionRegistry::default();
    let id = registry.reserve("p", 1, owner(), 2).unwrap();
    registry.update(&id, |entry| {
        entry.state = ExecutionState::Running;
        entry.observe(terminal());
        entry.observe(RetainedSessionStatus {
            state: RetainedSessionState::Running,
            ..terminal()
        });
    });
    registry.update(&id, |entry| {
        entry.observe(RetainedSessionStatus {
            output_complete: true,
            stdout_bytes: 4,
            ..terminal()
        })
    });
    let entry = registry.get(&id, "p", 1, &owner()).unwrap();
    assert!(
        matches!(&entry.state, ExecutionState::Terminal(status) if status.output_complete && status.stdout_bytes == 4 && status.exit_code == Some(7))
    );
    let descriptor = orchestration::descriptor(&id, &entry);
    assert!(orchestration::require_final(descriptor).is_ok());
    let running = registry.reserve("p", 1, owner(), 2).unwrap();
    registry.update(&running, |entry| entry.state = ExecutionState::Running);
    let running_entry = registry.get(&running, "p", 1, &owner()).unwrap();
    assert!(
        orchestration::require_final(orchestration::descriptor(&running, &running_entry)).is_err()
    );
    let uncertain = registry.reserve("p", 1, owner(), 2).unwrap();
    registry.update(&uncertain, |entry| {
        entry.mark_uncertain(None);
        entry.observe(terminal());
    });
    assert!(matches!(
        registry.get(&uncertain, "p", 1, &owner()).unwrap().state,
        ExecutionState::Uncertain
    ));
    registry.update(&uncertain, |entry| {
        entry.terminal_at = Some(Instant::now() - TERMINAL_TTL)
    });
    assert!(registry.get(&uncertain, "p", 1, &owner()).is_none());
}

#[test]
fn execution_dispatched_input_uncertainty_blocks_running_and_expires() {
    let registry = ExecutionRegistry::default();
    let id = registry.reserve("p", 1, owner(), 2).unwrap();
    registry.update(&id, |entry| entry.state = ExecutionState::Running);
    assert!(matches!(
        registry.get(&id, "p", 1, &owner()).unwrap().state,
        ExecutionState::Running
    ));
    registry.update(&id, |entry| entry.mark_uncertain(None));
    let entry = registry.get(&id, "p", 1, &owner()).unwrap();
    assert!(matches!(entry.state, ExecutionState::Uncertain));
    assert!(entry.terminal_at.is_some());
    registry.update(&id, |entry| {
        entry.terminal_at = Some(Instant::now() - TERMINAL_TTL)
    });
    assert!(registry.get(&id, "p", 1, &owner()).is_none());
}

#[test]
fn execution_output_encodes_lossless_bytes_and_counters() {
    assert!(matches!(
        OutputError::from(crate::tools::ToolError::NotFound(
            "execution not found".into()
        )),
        OutputError::Tool(crate::tools::ToolError::NotFound(_))
    ));
    let value: Output = runner_protocol::SessionOutputChunk {
        stdout: vec![0xff],
        stderr: vec![0, 1],
        next_cursor: "cursor".into(),
        eof: false,
        output_complete: false,
        trimmed: true,
        truncated: true,
        dropped_bytes: 4,
        stdout_bytes: 5,
        stderr_bytes: 6,
    }
    .into();
    assert_eq!(value.stdout_base64, "/w==");
    assert_eq!(value.stderr_base64, "AAE=");
    assert!(value.trimmed && value.truncated);
    assert_eq!(
        (value.dropped_bytes, value.stdout_bytes, value.stderr_bytes),
        (4, 5, 6)
    );
}

#[cfg(test)]
mod input_diagnostics {
    use std::{
        collections::HashMap,
        fmt,
        sync::{Arc, Mutex},
    };

    use tracing::{
        Event, Metadata, Subscriber,
        field::{Field, Visit},
        span,
    };

    use super::super::orchestration::{InputStage, warn_input_failure};

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<(String, String)>>>);

    impl Capture {
        fn fields(&self) -> HashMap<String, String> {
            std::mem::take(&mut *self.0.lock().expect("capture lock"))
                .into_iter()
                .collect()
        }
    }

    struct Visitor(Vec<(String, String)>);

    impl Visit for Visitor {
        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.push((field.name().to_owned(), value.to_owned()));
        }

        fn record_bool(&mut self, field: &Field, value: bool) {
            self.0.push((field.name().to_owned(), value.to_string()));
        }

        fn record_u64(&mut self, field: &Field, value: u64) {
            self.0.push((field.name().to_owned(), value.to_string()));
        }

        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            self.0.push((field.name().to_owned(), format!("{value:?}")));
        }
    }

    impl Subscriber for Capture {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &span::Attributes<'_>) -> span::Id {
            span::Id::from_u64(1)
        }

        fn record(&self, _span: &span::Id, _values: &span::Record<'_>) {}

        fn record_follows_from(&self, _span: &span::Id, _follows: &span::Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut visitor = Visitor(Vec::new());
            event.record(&mut visitor);
            self.0.lock().expect("capture lock").extend(visitor.0);
        }

        fn enter(&self, _span: &span::Id) {}

        fn exit(&self, _span: &span::Id) {}
    }

    fn capture(stage: InputStage) -> HashMap<String, String> {
        let capture = Capture::default();
        tracing::subscriber::with_default(capture.clone(), || {
            warn_input_failure(stage, std::time::Instant::now())
        });
        capture.fields()
    }

    #[test]
    fn input_diagnostic_stages_and_dispatch_attempted_are_fixed() {
        for (stage, name, attempted) in [
            (InputStage::Validation, "validation", false),
            (InputStage::Lookup, "lookup", false),
            (InputStage::State, "state", false),
            (InputStage::SessionMissing, "session_missing", false),
            (InputStage::FenceAdmit, "fence_admit", false),
            (InputStage::ManagerDispatch, "manager_dispatch", true),
            (InputStage::FenceCommit, "fence_commit", true),
        ] {
            assert_eq!(stage.as_str(), name);
            assert_eq!(stage.dispatch_attempted(), attempted, "{name}");
        }
    }

    #[test]
    fn fence_commit_event_is_one_fixed_warning_and_keeps_dispatch_attempted() {
        let fields = capture(InputStage::FenceCommit);
        assert_eq!(
            fields.get("stage").map(String::as_str),
            Some("fence_commit")
        );
        assert_eq!(
            fields.get("dispatch_attempted").map(String::as_str),
            Some("true")
        );
        assert!(fields.contains_key("elapsed_ms"), "{fields:?}");
        assert_eq!(
            fields.get("message").map(String::as_str),
            Some("workspace execution input failed")
        );
        assert_eq!(fields.len(), 4, "{fields:?}");
    }

    #[test]
    fn input_event_fields_cannot_carry_payload_sentinels() {
        let sentinel = "sentinel-input-handle-credential";
        let fields = capture(InputStage::ManagerDispatch);
        let rendered = format!("{fields:?}");
        assert!(!rendered.contains(sentinel), "{rendered}");
        for name in ["execution_id", "session_id", "chars", "principal", "error"] {
            assert!(!fields.contains_key(name), "{name} leaked: {fields:?}");
        }
    }
}
