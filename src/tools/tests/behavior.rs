use std::{path::PathBuf, sync::Arc};

use runner_protocol::{
    CancelSessionRequest, CommandOutput, ExecRequest, InputRequest, RunnerCommandRequest,
    RunnerOwner, RunnerSessionStatus, ShellToolOutput,
};
use serde_json::json;
use tempfile::tempdir;
use tokio::sync::Mutex;

use crate::{
    runner::{RunnerFuture, RunnerService},
    tools::{
        ToolError,
        common::{
            self, PatchCodeOutput, PatchDiagnosticOutput, PatchEffectOutput, PatchOperationOutput,
            PatchResultOutput, PatchStageOutput, PatchStatusOutput,
        },
        params::{
            ApplyPatchParams, GlobParams, GrepParams, ReadParams, ShellParams, WriteStdinParams,
        },
        readonly, shared,
    },
};
use desk_foreman_workspace_sdk::{
    ApplyPatchOperation, PatchApplied, PatchChange, PatchChangeError, PatchCode, PatchDiagnostic,
    PatchEffect, PatchIncomplete, PatchRejected, PatchResult, PatchStage, PatchWrites,
};

use super::{
    StaticCommandRunner, app_state, app_state_with_runner, parse_params, parse_tool_params,
    test_actor, top_level_keys,
};

#[test]
fn write_stdin_defaults_chars_to_empty_string() {
    let params: WriteStdinParams = parse_params(json!({ "session_id": 7 })).expect("params");
    assert_eq!(params.session_id, 7);
    assert!(params.chars.is_empty());
}

#[test]
fn apply_patch_requires_patch_field() {
    let params: ApplyPatchParams =
        parse_params(json!({ "patchText": "*** Begin Patch\n*** End Patch\n" }))
            .expect("patch should parse");
    assert!(params.patch_text.contains("*** Begin Patch"));
    parse_params::<ApplyPatchParams>(json!({ "body": "*** Begin Patch\n*** End Patch\n" }))
        .expect_err("missing patch field should fail");
}

#[tokio::test]
async fn protected_absolute_patch_path_is_not_echoed() {
    let temp = tempdir().expect("tempdir");
    let actor = test_actor(temp.path(), 10);
    let params: ApplyPatchParams = parse_params(json!({
        "patchText": "*** Begin Patch\n*** Add File: /private/.env\n+content\n*** End Patch"
    }))
    .expect("params");
    let output = shared::apply_patch(&app_state(temp.path().to_path_buf()), &actor, &params)
        .await
        .expect("domain rejection");
    let PatchResultOutput::Rejected(rejected) = output else {
        panic!("expected rejected result");
    };
    assert_eq!(rejected.diagnostic.path, None);
}

async fn patch_rejection(temp: &tempfile::TempDir, patch: &str) -> PatchDiagnosticOutput {
    let actor = test_actor(temp.path(), 10);
    let params: ApplyPatchParams = parse_params(json!({ "patchText": patch })).expect("params");
    let output = shared::apply_patch(&app_state(temp.path().to_path_buf()), &actor, &params)
        .await
        .expect("domain rejection");
    let PatchResultOutput::Rejected(rejected) = output else {
        panic!("expected rejected result");
    };
    rejected.diagnostic
}

#[tokio::test]
async fn invalid_patch_paths_are_content_free_before_admission() {
    let temp = tempdir().expect("tempdir");
    let outside = tempdir().expect("outside tempdir");
    std::fs::write(temp.path().join("notes.txt"), "one\n").expect("seed");
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), temp.path().join("link")).expect("symlink");

    let cases = [
        (
            "absolute add",
            "*** Begin Patch\n*** Add File: /private/.env\n+content\n*** End Patch",
            PatchOperationOutput::Add,
            "/private",
        ),
        (
            "absolute move destination",
            "*** Begin Patch\n*** Update File: notes.txt\n*** Move to: /private/escaped.txt\n@@\n-one\n+two\n*** End Patch",
            PatchOperationOutput::Move,
            "/private",
        ),
        (
            "traversal source",
            "*** Begin Patch\n*** Update File: ../escape.txt\n@@\n-one\n+two\n*** End Patch",
            PatchOperationOutput::Update,
            "../escape.txt",
        ),
        (
            "traversal move destination",
            "*** Begin Patch\n*** Update File: notes.txt\n*** Move to: ../escape.txt\n@@\n-one\n+two\n*** End Patch",
            PatchOperationOutput::Move,
            "../escape.txt",
        ),
        (
            "symlink escape source",
            "*** Begin Patch\n*** Add File: link/secret.txt\n+content\n*** End Patch",
            PatchOperationOutput::Add,
            "link/secret.txt",
        ),
        (
            "symlink escape move destination",
            "*** Begin Patch\n*** Update File: notes.txt\n*** Move to: link/secret.txt\n@@\n-one\n+two\n*** End Patch",
            PatchOperationOutput::Move,
            "link/secret.txt",
        ),
    ];

    for (label, patch, operation, raw_path) in cases {
        let diagnostic = patch_rejection(&temp, patch).await;
        assert_eq!(
            diagnostic.code,
            PatchCodeOutput::PatchPathInvalid,
            "{label} code"
        );
        assert_eq!(diagnostic.stage, PatchStageOutput::Plan, "{label} stage");
        assert_eq!(diagnostic.path, None, "{label} path must not be echoed");
        assert_eq!(
            diagnostic.operation,
            Some(operation),
            "{label} keeps the known operation"
        );
        assert_eq!(
            diagnostic.message, "patch path is not a valid workspace-relative path",
            "{label} message"
        );
        assert!(
            !serde_json::to_string(&diagnostic)
                .expect("diagnostic serializes")
                .contains(raw_path),
            "{label} must not echo the rejected path"
        );
    }

    assert!(!temp.path().join("escape.txt").exists());
    assert!(!outside.path().join("secret.txt").exists());
    assert_eq!(
        std::fs::read_to_string(temp.path().join("notes.txt")).unwrap(),
        "one\n",
        "invalid paths never mutate the workspace"
    );

    let protected = patch_rejection(
        &temp,
        "*** Begin Patch\n*** Add File: nested/.env\n+secret\n*** End Patch",
    )
    .await;
    assert_eq!(protected.code, PatchCodeOutput::PatchPathProtected);
    assert_eq!(protected.stage, PatchStageOutput::Plan);
    assert_eq!(protected.path.as_deref(), Some("nested/.env"));
    assert_eq!(protected.operation, Some(PatchOperationOutput::Add));
    assert_eq!(
        protected.message,
        "patch path is protected by workspace policy"
    );
    assert!(!temp.path().join("nested").exists());
}

#[tokio::test]
async fn shared_apply_patch_returns_structured_change_details() {
    let temp = tempdir().expect("tempdir");
    std::fs::write(temp.path().join("notes.txt"), "old\n").expect("seed");
    let actor = test_actor(temp.path(), 10);
    let params: ApplyPatchParams = parse_params(json!({
        "patchText": "*** Begin Patch\n*** Update File: notes.txt\n@@\n-old\n+new\n*** End Patch"
    }))
    .expect("params");

    let output = shared::apply_patch(&app_state(temp.path().to_path_buf()), &actor, &params)
        .await
        .expect("apply patch");

    let PatchResultOutput::Applied(applied) = output else {
        panic!("expected applied result");
    };
    let changes = applied.changes;
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].operation, PatchOperationOutput::Update);
    assert_eq!(changes[0].status, PatchStatusOutput::Applied);
    assert_eq!(changes[0].effect, PatchEffectOutput::Changed);
    assert!(changes[0].error.is_none());
    assert_eq!(
        std::fs::read_to_string(temp.path().join("notes.txt")).unwrap(),
        "new\n"
    );
}

#[test]
fn mcp_param_validation_rejects_blank_and_invalid_ranges() {
    let blank = parse_tool_params::<ShellParams>(json!({
        "command": "   "
    }))
    .expect_err("blank cmd should fail");
    assert!(blank.message.contains("command: must not be blank"));

    let range = parse_tool_params::<ReadParams>(json!({
        "filePath": "notes.txt",
        "offset": 0,
        "limit": 3
    }))
    .expect_err("reversed line range should fail");
    assert!(range.message.contains("offset"));
}

#[test]
fn mcp_empty_params_reject_unknown_fields() {
    parse_tool_params::<crate::tools::params::EmptyParams>(json!({
        "unexpected": true
    }))
    .expect_err("unknown field should fail");
}

#[tokio::test]
async fn readonly_tools_return_structured_content_with_expected_keys() {
    let temp = tempdir().expect("tempdir");
    std::fs::create_dir(temp.path().join("src")).expect("mkdir");
    std::fs::write(temp.path().join("src/sample.txt"), "alpha\nbeta\n").expect("write");

    let read = readonly::read_file_result(
        temp.path(),
        &parse_params(json!({
            "filePath": "src/sample.txt",
            "offset": 1,
            "limit": 1
        }))
        .expect("params"),
    )
    .expect("read result");
    assert_eq!(
        top_level_keys(read.structured_content.as_ref().expect("structured")),
        vec![
            "content",
            "entries",
            "kind",
            "offset",
            "path",
            "total",
            "truncated"
        ]
    );

    let directory = readonly::read_file_result(
        temp.path(),
        &parse_params(json!({ "filePath": "src" })).expect("params"),
    )
    .expect("directory read result");
    assert_eq!(
        top_level_keys(directory.structured_content.as_ref().expect("structured")),
        vec![
            "content",
            "entries",
            "kind",
            "offset",
            "path",
            "total",
            "truncated"
        ]
    );

    let search = readonly::search_files_result(
        temp.path(),
        &parse_params(json!({ "pattern": "beta", "path": "." })).expect("params"),
    )
    .await
    .expect("search result");
    assert_eq!(
        top_level_keys(search.structured_content.as_ref().expect("structured")),
        vec!["matches", "path", "pattern", "truncated"]
    );

    let stat = readonly::stat_path_result(
        temp.path(),
        &parse_params(json!({ "path": "src/sample.txt" })).expect("params"),
    )
    .expect("stat result");
    assert_eq!(
        top_level_keys(stat.structured_content.as_ref().expect("structured")),
        vec!["kind", "path", "readonly", "size"]
    );
}

#[tokio::test]
async fn shared_exec_command_returns_shell_output() {
    let temp = tempdir().expect("tempdir");
    let state = app_state(temp.path().to_path_buf());
    let actor = test_actor(temp.path(), 10);
    let params: ShellParams = parse_params(json!({
        "command": "printf 'hello'"
    }))
    .expect("params");

    let output = shared::shell(&state, &actor, &params)
        .await
        .expect("exec command");

    assert_eq!(output.output, "hello");
    assert_eq!(output.exit_code, Some(0));
    assert_eq!(output.session_id, None);
}

#[tokio::test]
async fn shared_shell_allows_git_commands() {
    let temp = tempdir().expect("tempdir");
    let state = app_state(temp.path().to_path_buf());
    let actor = test_actor(temp.path(), 10);
    let params: ShellParams = parse_params(json!({
        "command": "git status --short"
    }))
    .expect("params");

    let output = shared::shell(&state, &actor, &params)
        .await
        .expect("git command should use shell");

    assert_eq!(output.exit_code, Some(0));
}

#[tokio::test]
async fn shared_shell_rejects_sensitive_paths_and_dangerous_commands() {
    let temp = tempdir().expect("tempdir");
    let state = app_state(temp.path().to_path_buf());
    let actor = test_actor(temp.path(), 10);

    let sensitive = shared::shell(
        &state,
        &actor,
        &parse_params::<ShellParams>(json!({ "command": "cat .env" })).expect("params"),
    )
    .await
    .expect_err("sensitive path should be denied");
    assert!(matches!(sensitive, ToolError::Forbidden(_)));

    let dangerous = shared::shell(
        &state,
        &actor,
        &parse_params::<ShellParams>(json!({ "command": "docker ps" })).expect("params"),
    )
    .await
    .expect_err("docker command should be denied");
    assert!(matches!(dangerous, ToolError::Forbidden(_)));
}

#[tokio::test]
async fn shared_write_stdin_rejects_foreign_session() {
    let temp = tempdir().expect("tempdir");
    let state = app_state(temp.path().to_path_buf());
    let owner = test_actor(temp.path(), 10);
    let intruder = test_actor(temp.path(), 11);
    let exec_params: ShellParams = parse_params(json!({
        "command": "cat"
    }))
    .expect("params");
    let session = shared::shell(&state, &owner, &exec_params)
        .await
        .expect("exec");

    let error = shared::write_stdin(
        &state,
        &intruder,
        &parse_params::<WriteStdinParams>(json!({
            "session_id": session.session_id.expect("session id"),
            "chars": "hello\n"
        }))
        .expect("write params"),
    )
    .await
    .expect_err("foreign session should fail");

    assert!(matches!(&error, ToolError::NotFound(message) if message == "session not found"));

    let result = common::tool_error_result(error).expect("tool-level error result");
    let payload = result.structured_content.expect("structured error");
    assert_eq!(result.is_error, Some(true));
    assert_eq!(payload["error"]["code"], "not_found");
    assert_eq!(payload["error"]["repairable"], true);
    assert_eq!(payload["error"]["retryable"], false);
}

#[tokio::test]
async fn shared_read_file_returns_typed_output() {
    let temp = tempdir().expect("tempdir");
    std::fs::write(temp.path().join("notes.txt"), "alpha\nbeta\n").expect("write");
    let actor = test_actor(temp.path(), 10);
    let params: ReadParams = parse_params(json!({
        "filePath": "notes.txt",
        "offset": 2,
        "limit": 1
    }))
    .expect("params");

    let state = app_state(temp.path().to_path_buf());
    let output = shared::read(&state, &actor, &params).expect("read");

    assert_eq!(output.path, "notes.txt");
    assert_eq!(output.content.as_deref(), Some("beta\n"));
    assert!(!output.truncated);
}

#[tokio::test]
async fn missing_read_path_is_a_repairable_tool_error() {
    let temp = tempdir().expect("tempdir");
    let actor = test_actor(temp.path(), 10);
    let params: ReadParams = parse_params(json!({
        "filePath": "missing.rs"
    }))
    .expect("params");

    let error = shared::read(&app_state(temp.path().to_path_buf()), &actor, &params)
        .expect_err("missing path should fail");
    assert!(matches!(&error, ToolError::NotFound(_)));

    let result = common::tool_error_result(error).expect("tool-level error result");
    let payload = result.structured_content.expect("structured error");
    assert_eq!(result.is_error, Some(true));
    assert_eq!(payload["error"]["code"], "not_found");
    assert_eq!(payload["error"]["repairable"], true);
    assert_eq!(payload["error"]["retryable"], false);
}

#[test]
fn forbidden_tool_errors_are_visible_without_being_retriable() {
    let result = common::tool_error_result(ToolError::Forbidden(
        "path is protected by workspace policy".to_string(),
    ))
    .expect("tool-level error result");
    let payload = result.structured_content.expect("structured error");

    assert_eq!(result.is_error, Some(true));
    assert_eq!(payload["error"]["category"], "policy");
    assert_eq!(payload["error"]["repairable"], false);
    assert_eq!(payload["error"]["retryable"], false);
}

#[tokio::test]
async fn shared_search_files_returns_structured_output() {
    let temp = tempdir().expect("tempdir");
    let state = app_state(temp.path().to_path_buf());
    let actor = test_actor(temp.path(), 10);

    std::fs::write(temp.path().join("search.txt"), "alpha\nbeta\n").expect("write");
    let search = shared::grep(
        &state,
        &actor,
        &parse_params::<GrepParams>(json!({
            "pattern": "beta",
            "path": "."
        }))
        .expect("search params"),
    )
    .await
    .expect("search");
    assert_eq!(search.matches.len(), 1);
    assert_eq!(search.matches[0].path, "search.txt");
}

#[tokio::test]
async fn grep_mixed_stdout_stderr_is_isolated() {
    let temp = tempdir().expect("tempdir");
    let actor = test_actor(temp.path(), 10);
    let valid_line = "{\"type\":\"match\",\"data\":{\"path\":{\"text\":\"search.txt\"},\"lines\":{\"text\":\"hello\\n\"},\"line_number\":1}}\n";
    let stderr = "warning: ignored directory\n";
    let output = CommandOutput {
        stdout: valid_line.to_string(),
        stderr: stderr.to_string(),
        output: format!("{valid_line}{stderr}"),
        exit_code: Some(0),
        truncated: false,
        timed_out: false,
        stdout_bytes: valid_line.len(),
        stderr_bytes: stderr.len(),
        wall_time_seconds: 0.01,
    };
    let state = app_state_with_runner(
        temp.path().to_path_buf(),
        Arc::new(StaticCommandRunner(output)),
    );
    let search = shared::grep(
        &state,
        &actor,
        &parse_params::<GrepParams>(json!({
            "pattern": "hello",
            "path": "."
        }))
        .expect("params"),
    )
    .await
    .expect("grep should succeed using stdout only");
    assert_eq!(search.matches.len(), 1);
    assert_eq!(search.matches[0].path, "search.txt");
    assert_eq!(search.matches[0].line, "hello");
}

#[tokio::test]
async fn grep_empty_no_match_returns_empty() {
    let temp = tempdir().expect("tempdir");
    let actor = test_actor(temp.path(), 10);
    let output = CommandOutput {
        stdout: String::new(),
        stderr: String::new(),
        output: String::new(),
        exit_code: Some(1),
        truncated: false,
        timed_out: false,
        stdout_bytes: 0,
        stderr_bytes: 0,
        wall_time_seconds: 0.01,
    };
    let state = app_state_with_runner(
        temp.path().to_path_buf(),
        Arc::new(StaticCommandRunner(output)),
    );
    let search = shared::grep(
        &state,
        &actor,
        &parse_params::<GrepParams>(json!({
            "pattern": "missing",
            "path": "."
        }))
        .expect("params"),
    )
    .await
    .expect("rg exit 1 should be treated as empty");
    assert!(search.matches.is_empty());
    assert!(!search.truncated);
}

#[tokio::test]
async fn grep_non_one_failure_reports_meaningful_error() {
    let temp = tempdir().expect("tempdir");
    let actor = test_actor(temp.path(), 10);
    let stderr = "rg: invalid regex";
    let output = CommandOutput {
        stdout: String::new(),
        stderr: stderr.to_string(),
        output: stderr.to_string(),
        exit_code: Some(2),
        truncated: false,
        timed_out: false,
        stdout_bytes: 0,
        stderr_bytes: stderr.len(),
        wall_time_seconds: 0.01,
    };
    let state = app_state_with_runner(
        temp.path().to_path_buf(),
        Arc::new(StaticCommandRunner(output)),
    );
    let error = shared::grep(
        &state,
        &actor,
        &parse_params::<GrepParams>(json!({
            "pattern": "[",
            "path": "."
        }))
        .expect("params"),
    )
    .await
    .expect_err("rg exit 2 should be tool error");
    assert!(matches!(error, ToolError::Internal(_)));
    let message = error.to_string();
    assert!(
        message.contains("rg failed"),
        "expected rg failure message, got {message}"
    );
    assert!(
        message.contains("2"),
        "expected exit code in message, got {message}"
    );
    assert!(
        !message.contains("expected value at line 1"),
        "should not be JSON parse error, got {message}"
    );
}

#[tokio::test]
async fn glob_stderr_is_not_treated_as_path() {
    let temp = tempdir().expect("tempdir");
    let actor = test_actor(temp.path(), 10);
    let stdout = "a.txt\nb.txt\n";
    let stderr = "warning: ignored .env\n";
    let output = CommandOutput {
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
        output: format!("{stdout}{stderr}"),
        exit_code: Some(0),
        truncated: false,
        timed_out: false,
        stdout_bytes: stdout.len(),
        stderr_bytes: stderr.len(),
        wall_time_seconds: 0.01,
    };
    let state = app_state_with_runner(
        temp.path().to_path_buf(),
        Arc::new(StaticCommandRunner(output)),
    );
    let result = shared::glob(
        &state,
        &actor,
        &parse_params::<GlobParams>(json!({
            "pattern": "*.txt",
            "path": "."
        }))
        .expect("params"),
    )
    .await
    .expect("glob should succeed");
    assert_eq!(result.matches, vec!["a.txt", "b.txt"]);
    assert!(
        !result.matches.iter().any(|p| p.contains("warning")),
        "stderr should not become path"
    );
    // truncation should be based on stdout only, not combined output
    assert!(!result.truncated);
}

#[tokio::test]
async fn patch_context_not_found_is_a_domain_result() {
    let temp = tempdir().expect("tempdir");
    std::fs::write(temp.path().join("a.txt"), "hello\nworld\n").expect("seed");
    let actor = test_actor(temp.path(), 10);
    let state = app_state(temp.path().to_path_buf());
    let params: ApplyPatchParams = parse_params(json!({
        "patchText": "*** Begin Patch\n*** Update File: a.txt\n@@\n-missing context\n+new\n*** End Patch"
    }))
    .expect("params");
    let output = shared::apply_patch(&state, &actor, &params)
        .await
        .expect("domain result, not a transport error");
    let PatchResultOutput::Rejected(rejected) = &output else {
        panic!("expected rejected result");
    };
    let diagnostic = &rejected.diagnostic;
    assert_eq!(diagnostic.code, PatchCodeOutput::PatchContextNotFound);
    assert_eq!(diagnostic.stage, PatchStageOutput::Plan);
    assert_eq!(diagnostic.path.as_deref(), Some("a.txt"));
    assert_eq!(diagnostic.operation, Some(PatchOperationOutput::Update));
    assert_eq!(diagnostic.hunk, Some(0));
    assert_eq!(diagnostic.message, "patch context was not found");
    let call = common::patch_call_result(&output).expect("patch call result");
    assert_eq!(call.is_error, Some(true));
    let structured = call.structured_content.clone().expect("structured");
    let text: serde_json::Value = serde_json::from_str(
        &call
            .content
            .first()
            .and_then(|block| block.as_text())
            .map(|text| text.text.clone())
            .expect("json text"),
    )
    .expect("consistent json text");
    assert_eq!(text, structured);
    // ensure generic invalid_input still maps to generic code for other failures
    let generic = common::tool_error_result(ToolError::InvalidInput("bad patch".to_string()))
        .expect("generic");
    let generic_payload = generic.structured_content.expect("structured");
    assert_eq!(generic_payload["error"]["code"], "invalid_input");
}

#[tokio::test]
async fn patch_applied_result_keeps_operation_effect_and_status() {
    let temp = tempdir().expect("tempdir");
    std::fs::write(temp.path().join("first.txt"), "one\n").expect("seed");
    std::fs::write(temp.path().join("second.txt"), "two\n").expect("seed");
    let actor = test_actor(temp.path(), 10);
    let state = app_state(temp.path().to_path_buf());
    let params: ApplyPatchParams = parse_params(json!({
        "patchText": "*** Begin Patch\n*** Update File: first.txt\n@@\n-one\n+updated\n*** Update File: second.txt\n@@\n-two\n+changed\n*** End Patch"
    }))
    .expect("params");
    let output = shared::apply_patch(&state, &actor, &params)
        .await
        .expect("both files patch should succeed");
    let PatchResultOutput::Applied(applied) = &output else {
        panic!("expected applied result");
    };
    let changes = &applied.changes;
    assert_eq!(changes.len(), 2);
    assert!(
        changes
            .iter()
            .all(|change| change.status == PatchStatusOutput::Applied)
    );
    assert!(
        changes
            .iter()
            .all(|change| change.effect == PatchEffectOutput::Changed && change.error.is_none())
    );
    let call = common::patch_call_result(&output).expect("patch call result");
    assert_eq!(call.is_error, Some(false));
    assert_eq!(
        std::fs::read_to_string(temp.path().join("first.txt")).unwrap(),
        "updated\n"
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("second.txt")).unwrap(),
        "changed\n"
    );
}

fn sample_patch_results() -> Vec<(&'static str, PatchResult)> {
    let applied = PatchChange {
        operation: ApplyPatchOperation::Update,
        path: "notes.txt".to_string(),
        destination: None,
        status: desk_foreman_workspace_sdk::ApplyPatchStatus::Applied,
        effect: PatchEffect::Changed,
        error: None,
    };
    let failed = PatchChange {
        path: "failed.txt".to_string(),
        status: desk_foreman_workspace_sdk::ApplyPatchStatus::Failed,
        effect: PatchEffect::Unchanged,
        error: Some(PatchChangeError::new(PatchCode::PatchWriteFailed)),
        ..applied.clone()
    };
    let skipped = PatchChange {
        path: "skipped.txt".to_string(),
        status: desk_foreman_workspace_sdk::ApplyPatchStatus::Skipped,
        effect: PatchEffect::Unchanged,
        error: None,
        ..applied.clone()
    };
    let noop = PatchChange {
        path: "noop.txt".to_string(),
        effect: PatchEffect::Unchanged,
        ..applied.clone()
    };
    let residue_failed = PatchChange {
        operation: ApplyPatchOperation::Add,
        path: "created/new.txt".to_string(),
        destination: None,
        status: desk_foreman_workspace_sdk::ApplyPatchStatus::Failed,
        effect: PatchEffect::Changed,
        error: Some(PatchChangeError::new(PatchCode::PatchWriteFailed)),
    };
    let residue_diagnostic = PatchDiagnostic::new(PatchCode::PatchWriteFailed, PatchStage::Commit)
        .with_path(Some("created/new.txt".to_string()))
        .with_operation(Some(ApplyPatchOperation::Add));
    let diagnostic = PatchDiagnostic::new(PatchCode::PatchContextNotFound, PatchStage::Plan)
        .with_path(Some("notes.txt".to_string()))
        .with_operation(Some(ApplyPatchOperation::Update))
        .with_hunk(Some(0));
    let commit = PatchDiagnostic::new(PatchCode::PatchTargetChanged, PatchStage::Commit)
        .with_path(Some("notes.txt".to_string()))
        .with_operation(Some(ApplyPatchOperation::Update));
    vec![
        (
            "applied",
            PatchResult::Applied(PatchApplied {
                changes: vec![applied.clone()],
            }),
        ),
        (
            "rejected",
            PatchResult::Rejected(PatchRejected {
                diagnostic: diagnostic.clone(),
            }),
        ),
        (
            "none",
            PatchResult::Incomplete(PatchIncomplete {
                writes: PatchWrites::None,
                changes: vec![failed.clone(), skipped.clone()],
                diagnostic: commit.clone(),
            }),
        ),
        (
            "all",
            PatchResult::Incomplete(PatchIncomplete {
                writes: PatchWrites::All,
                changes: vec![applied.clone()],
                diagnostic: commit.clone(),
            }),
        ),
        (
            "partial",
            PatchResult::Incomplete(PatchIncomplete {
                writes: PatchWrites::Partial,
                changes: vec![applied.clone(), failed.clone(), skipped.clone()],
                diagnostic: commit.clone(),
            }),
        ),
        (
            "unknown",
            PatchResult::Incomplete(PatchIncomplete {
                writes: PatchWrites::Unknown,
                changes: vec![
                    PatchChange {
                        effect: PatchEffect::Unknown,
                        ..failed.clone()
                    },
                    skipped,
                ],
                diagnostic: commit,
            }),
        ),
        (
            "fence_unchanged",
            shared::fence_failure_result(PatchResult::Applied(PatchApplied {
                changes: vec![noop],
            })),
        ),
        (
            "residue_failed_add",
            PatchResult::Incomplete(PatchIncomplete {
                writes: PatchWrites::Partial,
                changes: vec![residue_failed],
                diagnostic: residue_diagnostic,
            }),
        ),
    ]
}

#[test]
fn fence_failure_classifies_noop_and_changed_writes_truthfully() {
    let unchanged = PatchChange {
        operation: ApplyPatchOperation::Update,
        path: "noop.txt".to_string(),
        destination: None,
        status: desk_foreman_workspace_sdk::ApplyPatchStatus::Applied,
        effect: PatchEffect::Unchanged,
        error: None,
    };
    let changed = PatchChange {
        path: "changed.txt".to_string(),
        effect: PatchEffect::Changed,
        ..unchanged.clone()
    };

    for (label, changes, expected) in [
        ("noop", vec![unchanged.clone()], PatchWrites::None),
        ("changed", vec![changed.clone()], PatchWrites::All),
        ("mixed", vec![changed, unchanged], PatchWrites::All),
    ] {
        let result = shared::fence_failure_result(PatchResult::Applied(PatchApplied { changes }));
        let PatchResult::Incomplete(incomplete) = result else {
            panic!("{label} fence failure must be incomplete");
        };
        assert_eq!(incomplete.writes, expected, "{label} write classification");
        assert_eq!(
            incomplete.diagnostic.code,
            PatchCode::PatchFenceFailed,
            "{label} keeps the fence diagnostic"
        );
        assert_eq!(incomplete.diagnostic.stage, PatchStage::Fence);
    }
}

type DiagnosticSample = (
    PatchCode,
    PatchStage,
    Option<&'static str>,
    Option<ApplyPatchOperation>,
    Option<u32>,
);

/// Every fixed diagnostic code with a canonical stage and nullable fields.
/// The values come from the real SDK constructor, never a copied message table.
fn sample_patch_diagnostics() -> Vec<DiagnosticSample> {
    vec![
        (
            PatchCode::PatchArgInvalid,
            PatchStage::Params,
            None,
            None,
            None,
        ),
        (
            PatchCode::PatchTooLarge,
            PatchStage::Params,
            None,
            None,
            None,
        ),
        (
            PatchCode::PatchBoundaryInvalid,
            PatchStage::Parse,
            None,
            None,
            None,
        ),
        (
            PatchCode::PatchHunkInvalid,
            PatchStage::Parse,
            None,
            None,
            None,
        ),
        (
            PatchCode::PatchPathInvalid,
            PatchStage::Plan,
            Some("notes.txt"),
            Some(ApplyPatchOperation::Update),
            None,
        ),
        (
            PatchCode::PatchPathProtected,
            PatchStage::Plan,
            Some(".git/config"),
            Some(ApplyPatchOperation::Update),
            None,
        ),
        (
            PatchCode::PatchTargetMissing,
            PatchStage::Plan,
            Some("missing.txt"),
            Some(ApplyPatchOperation::Update),
            None,
        ),
        (
            PatchCode::PatchTargetExists,
            PatchStage::Plan,
            Some("existing.txt"),
            Some(ApplyPatchOperation::Add),
            None,
        ),
        (
            PatchCode::PatchTargetNotRegular,
            PatchStage::Plan,
            Some("directory"),
            Some(ApplyPatchOperation::Update),
            None,
        ),
        (
            PatchCode::PatchNotUtf8Text,
            PatchStage::Plan,
            Some("binary.bin"),
            Some(ApplyPatchOperation::Update),
            None,
        ),
        (
            PatchCode::PatchContextNotFound,
            PatchStage::Plan,
            Some("notes.txt"),
            Some(ApplyPatchOperation::Update),
            Some(2),
        ),
        (
            PatchCode::PatchContextAmbiguous,
            PatchStage::Plan,
            Some("notes.txt"),
            Some(ApplyPatchOperation::Update),
            Some(1),
        ),
        (
            PatchCode::PatchMoveConflict,
            PatchStage::Plan,
            Some("moved.txt"),
            Some(ApplyPatchOperation::Move),
            None,
        ),
        (
            PatchCode::PatchDuplicateTarget,
            PatchStage::Plan,
            Some("notes.txt"),
            Some(ApplyPatchOperation::Update),
            None,
        ),
        (
            PatchCode::PatchTargetChanged,
            PatchStage::Commit,
            Some("notes.txt"),
            Some(ApplyPatchOperation::Update),
            None,
        ),
        (
            PatchCode::PatchSourceChanged,
            PatchStage::Commit,
            Some("notes.txt"),
            Some(ApplyPatchOperation::Move),
            None,
        ),
        (
            PatchCode::PatchWriteFailed,
            PatchStage::Commit,
            Some("notes.txt"),
            Some(ApplyPatchOperation::Update),
            None,
        ),
        (
            PatchCode::PatchFenceFailed,
            PatchStage::Fence,
            None,
            None,
            None,
        ),
    ]
}

fn diagnostic_from(sample: &DiagnosticSample) -> PatchDiagnostic {
    PatchDiagnostic::new(sample.0, sample.1)
        .with_path(sample.2.map(str::to_string))
        .with_operation(sample.3)
        .with_hunk(sample.4)
}

#[test]
fn all_eighteen_sdk_diagnostics_match_schema_facing_output() {
    let samples = sample_patch_diagnostics();
    assert_eq!(
        samples.len(),
        18,
        "the fixed diagnostic code set has 18 codes"
    );
    let mut seen = std::collections::BTreeSet::new();
    for sample in &samples {
        let diagnostic = diagnostic_from(sample);
        let code = serde_json::to_string(&diagnostic.code).expect("code serializes");
        assert!(seen.insert(code), "duplicate diagnostic code");
        assert_eq!(diagnostic.message(), diagnostic.code.message());
        let sdk = serde_json::to_value(&diagnostic).expect("sdk diagnostic serializes");
        let output = PatchDiagnosticOutput::from(diagnostic);
        let facing = serde_json::to_value(&output).expect("schema-facing diagnostic serializes");
        assert_eq!(sdk, facing, "SDK and schema-facing diagnostics differ");
    }
    assert_eq!(seen.len(), 18);
}

#[test]
fn sdk_result_serde_matches_schema_facing_output() {
    let catalog = crate::tools::DeskForemanService::tool_catalog();
    let schema = catalog
        .iter()
        .find(|tool| tool.name == "apply_patch")
        .and_then(|tool| tool.output_schema.as_ref())
        .expect("apply_patch output schema");
    for (label, result) in sample_patch_results() {
        let sdk = serde_json::to_value(&result).expect("sdk result serializes");
        let output: PatchResultOutput = result.into();
        let facing = serde_json::to_value(&output).expect("output serializes");
        assert_eq!(sdk, facing, "{label} SDK and schema-facing values differ");
        let outcome = facing["outcome"].as_str().expect("outcome");
        let variant = schema["oneOf"]
            .as_array()
            .expect("variants")
            .iter()
            .find(|variant| variant["properties"]["outcome"]["const"] == outcome)
            .expect("serving variant");
        let keys = facing
            .as_object()
            .expect("object")
            .keys()
            .collect::<std::collections::BTreeSet<_>>();
        let schema_keys = variant["properties"]
            .as_object()
            .expect("schema properties")
            .keys()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(keys, schema_keys, "{label} keys do not match schema");
    }
}

#[test]
#[ignore = "writes the cross-repo patch result serializer fixture"]
fn export_patch_result_fixtures_for_stock() {
    let mut fixture = serde_json::Map::new();
    for (label, result) in sample_patch_results() {
        let output: PatchResultOutput = result.into();
        fixture.insert(
            label.to_string(),
            serde_json::to_value(output).expect("fixture serializes"),
        );
    }
    fixture.insert(
        "change_error".to_string(),
        serde_json::to_value(PatchChange {
            operation: ApplyPatchOperation::Update,
            path: "notes.txt".to_string(),
            destination: None,
            status: desk_foreman_workspace_sdk::ApplyPatchStatus::Failed,
            effect: PatchEffect::Unchanged,
            error: Some(PatchChangeError::new(PatchCode::PatchWriteFailed)),
        })
        .expect("change error serializes"),
    );

    let catalog = crate::tools::DeskForemanService::tool_catalog();
    let apply_patch = catalog
        .iter()
        .find(|tool| tool.name == "apply_patch")
        .expect("apply_patch registered");
    let schema = apply_patch
        .output_schema
        .as_ref()
        .expect("apply_patch output schema");
    let variants = schema
        .get("oneOf")
        .and_then(serde_json::Value::as_array)
        .expect("strict result variants");
    assert_eq!(variants.len(), 3);
    for (label, value) in &fixture {
        if label == "change_error" {
            continue;
        }
        let outcome = value["outcome"].as_str().expect("variant outcome");
        let variant = variants
            .iter()
            .find(|variant| variant["properties"]["outcome"]["const"] == outcome)
            .expect("serving variant");
        let keys = value
            .as_object()
            .expect("fixture object")
            .keys()
            .collect::<std::collections::BTreeSet<_>>();
        let schema_keys = variant["properties"]
            .as_object()
            .expect("schema properties")
            .keys()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            keys, schema_keys,
            "{label} fixture keys match serving schema"
        );
    }

    let openapi = crate::api::openapi_document();
    let schemas = openapi.components.expect("components").schemas;
    assert!(schemas.contains_key("PatchResultOutput"));
    std::fs::create_dir_all(".cache/ai/1173-p2-attempt3").expect("scratch directory");
    std::fs::write(
        ".cache/ai/1173-p2-attempt3/patch-result-fixtures.json",
        serde_json::to_string_pretty(&serde_json::Value::Object(fixture)).expect("pretty fixture"),
    )
    .expect("write fixture");
}

#[test]
#[ignore = "writes the cross-repo all-code diagnostic fixture"]
fn export_patch_diagnostics_for_stock() {
    let mut fixture = serde_json::Map::new();
    for sample in sample_patch_diagnostics() {
        let diagnostic = diagnostic_from(&sample);
        let value = serde_json::to_value(&diagnostic).expect("diagnostic serializes");
        let code = value["code"]
            .as_str()
            .expect("diagnostic code string")
            .to_string();
        fixture.insert(code, value);
    }
    assert_eq!(fixture.len(), 18);
    std::fs::create_dir_all(".cache/ai/1173-p4").expect("scratch directory");
    std::fs::write(
        ".cache/ai/1173-p4/patch-diagnostics.json",
        serde_json::to_string_pretty(&serde_json::Value::Object(fixture))
            .expect("pretty diagnostics fixture"),
    )
    .expect("write diagnostics fixture");
}

struct CapturingCommandRunner {
    requests: Mutex<Vec<RunnerCommandRequest>>,
    output: CommandOutput,
}

impl CapturingCommandRunner {
    fn new(output: CommandOutput) -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
            output,
        })
    }

    async fn last_request(&self) -> RunnerCommandRequest {
        self.requests
            .lock()
            .await
            .last()
            .expect("runner should receive a command")
            .clone()
    }
}

impl RunnerService for CapturingCommandRunner {
    fn exec_shell<'a>(
        &'a self,
        _request: ExecRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async move { anyhow::bail!("unsupported") })
    }

    fn write_stdin<'a>(
        &'a self,
        _request: InputRequest,
    ) -> RunnerFuture<'a, anyhow::Result<ShellToolOutput>> {
        Box::pin(async move { anyhow::bail!("unsupported") })
    }

    fn run_command<'a>(
        &'a self,
        request: RunnerCommandRequest,
    ) -> RunnerFuture<'a, anyhow::Result<CommandOutput>> {
        Box::pin(async move {
            self.requests.lock().await.push(request);
            Ok(self.output.clone())
        })
    }

    fn cancel_session<'a>(
        &'a self,
        _request: CancelSessionRequest,
    ) -> RunnerFuture<'a, anyhow::Result<RunnerSessionStatus>> {
        Box::pin(async move { anyhow::bail!("unsupported") })
    }

    fn list_sessions<'a>(&'a self) -> RunnerFuture<'a, anyhow::Result<Vec<RunnerSessionStatus>>> {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn cleanup_runner_owner<'a>(
        &'a self,
        _owner: RunnerOwner,
    ) -> RunnerFuture<'a, anyhow::Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

fn rg_match_output(path: &str, line: &str, line_number: u64) -> CommandOutput {
    let stdout = format!(
        "{{\"type\":\"match\",\"data\":{{\"path\":{{\"text\":\"{path}\"}},\"lines\":{{\"text\":\"{line}\\n\"}},\"line_number\":{line_number}}}}}\n"
    );
    CommandOutput {
        output: stdout.clone(),
        stdout: stdout.clone(),
        stdout_bytes: stdout.len(),
        stderr: String::new(),
        stderr_bytes: 0,
        exit_code: Some(0),
        truncated: false,
        timed_out: false,
        wall_time_seconds: 0.01,
    }
}

#[tokio::test]
async fn grep_file_path_uses_parent_as_workdir() {
    let temp = tempdir().expect("tempdir");
    std::fs::create_dir_all(temp.path().join("src")).expect("mkdir src");
    std::fs::write(temp.path().join("src/lib.rs"), "market\n").expect("seed");
    let actor = test_actor(temp.path(), 10);
    let runner = CapturingCommandRunner::new(rg_match_output("lib.rs", "market", 1));
    let state = app_state_with_runner(temp.path().to_path_buf(), runner.clone());

    let search = shared::grep(
        &state,
        &actor,
        &parse_params::<GrepParams>(json!({
            "pattern": "market",
            "path": "src/lib.rs"
        }))
        .expect("params"),
    )
    .await
    .expect("file grep should succeed");

    let request = runner.last_request().await;
    let expected_workdir: PathBuf = actor.workspace_root.join("src");
    assert_eq!(request.working_dir, expected_workdir);
    assert_eq!(request.program, "rg");
    assert!(
        request.args.iter().any(|arg| arg == "market"),
        "pattern should be in rg args: {:?}",
        request.args
    );
    assert_eq!(
        request.args.last().map(String::as_str),
        Some("lib.rs"),
        "file name should be the explicit rg target: {:?}",
        request.args
    );
    assert_eq!(search.matches.len(), 1);
    assert_eq!(search.matches[0].path, "src/lib.rs");
    assert_eq!(search.path, "src/lib.rs");
}

#[tokio::test]
async fn grep_directory_path_keeps_directory_as_workdir() {
    let temp = tempdir().expect("tempdir");
    std::fs::create_dir_all(temp.path().join("src")).expect("mkdir src");
    std::fs::write(temp.path().join("src/lib.rs"), "market\n").expect("seed");
    let actor = test_actor(temp.path(), 10);
    let runner = CapturingCommandRunner::new(rg_match_output("lib.rs", "market", 1));
    let state = app_state_with_runner(temp.path().to_path_buf(), runner.clone());

    let search = shared::grep(
        &state,
        &actor,
        &parse_params::<GrepParams>(json!({
            "pattern": "market",
            "path": "src"
        }))
        .expect("params"),
    )
    .await
    .expect("directory grep should succeed");

    let request = runner.last_request().await;
    let expected_workdir: PathBuf = actor.workspace_root.join("src");
    assert_eq!(request.working_dir, expected_workdir);
    assert_eq!(
        request.args.last().map(String::as_str),
        Some("market"),
        "directory search should not add an explicit file target: {:?}",
        request.args
    );
    assert_eq!(search.matches.len(), 1);
    assert_eq!(search.matches[0].path, "src/lib.rs");
    assert_eq!(search.path, "src");
}

#[tokio::test]
async fn grep_missing_path_returns_not_found() {
    let temp = tempdir().expect("tempdir");
    let actor = test_actor(temp.path(), 10);
    let runner = CapturingCommandRunner::new(rg_match_output("missing.txt", "market", 1));
    let state = app_state_with_runner(temp.path().to_path_buf(), runner);

    let error = shared::grep(
        &state,
        &actor,
        &parse_params::<GrepParams>(json!({
            "pattern": "market",
            "path": "missing.txt"
        }))
        .expect("params"),
    )
    .await
    .expect_err("missing path should fail");
    assert!(
        matches!(error, ToolError::NotFound(_)),
        "expected not_found, got {error:?}"
    );
}
