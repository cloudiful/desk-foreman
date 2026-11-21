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
