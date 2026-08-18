use serde_json::json;
use tempfile::tempdir;

use crate::tools::{
    ToolError, common,
    params::{ApplyPatchParams, EditParams, GrepParams, ReadParams, ShellParams, WriteStdinParams},
    readonly, shared,
};

use super::{app_state, parse_params, parse_tool_params, test_actor, top_level_keys};

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

    assert_eq!(output.summary, "U notes.txt");
    assert!(!output.partial);
    assert_eq!(output.changes.len(), 1);
    assert_eq!(output.changes[0].operation, "update");
    assert_eq!(output.changes[0].status, "applied");
    assert_eq!(output.changes[0].added_lines, 1);
    assert_eq!(output.changes[0].deleted_lines, 1);
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

#[tokio::test]
async fn edit_missing_path_is_a_repairable_tool_error() {
    let temp = tempdir().expect("tempdir");
    let actor = test_actor(temp.path(), 10);
    let params: EditParams = parse_params(json!({
        "path": "missing.rs",
        "old_text": "old",
        "new_text": "new"
    }))
    .expect("params");

    let error = shared::edit(&app_state(temp.path().to_path_buf()), &actor, &params)
        .await
        .expect_err("missing path should fail");
    assert!(matches!(&error, ToolError::NotFound(_)));

    let result = common::tool_error_result(error).expect("tool-level error result");
    let payload = result.structured_content.expect("structured error");
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
