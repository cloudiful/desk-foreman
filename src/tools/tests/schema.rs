use std::collections::BTreeMap;

use serde_json::{Value, json};
use tempfile::tempdir;

use crate::{
    api::openapi_document,
    tools::{DeskForemanService, params::ShellParams},
};

use super::{app_state, parse_params};

fn tool_signature_snapshot(service: &DeskForemanService) -> String {
    let mut snapshot = BTreeMap::new();
    for tool in service.tool_router.list_all() {
        let properties = tool
            .input_schema
            .get("properties")
            .and_then(Value::as_object)
            .map(|props| props.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let required = tool
            .input_schema
            .get("required")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        snapshot.insert(
            tool.name,
            json!({ "properties": properties, "required": required }),
        );
    }
    serde_json::to_string_pretty(&snapshot).expect("snapshot should serialize")
}

#[tokio::test]
async fn tool_schemas_match_expected_surface() {
    let temp = tempdir().expect("tempdir");
    let service = DeskForemanService::new(app_state(temp.path().to_path_buf()));
    assert_eq!(
        tool_signature_snapshot(&service),
        r#"{
  "apply_patch": {
    "properties": [
      "patchText"
    ],
    "required": [
      "patchText"
    ]
  },
  "cancel_session": {
    "properties": [
      "session_id"
    ],
    "required": [
      "session_id"
    ]
  },
  "glob": {
    "properties": [
      "path",
      "pattern"
    ],
    "required": [
      "pattern"
    ]
  },
  "grep": {
    "properties": [
      "include",
      "path",
      "pattern"
    ],
    "required": [
      "pattern"
    ]
  },
  "read": {
    "properties": [
      "filePath",
      "limit",
      "offset"
    ],
    "required": [
      "filePath"
    ]
  },
  "shell": {
    "properties": [
      "command",
      "max_output_tokens",
      "timeout",
      "workdir"
    ],
    "required": [
      "command"
    ]
  },
  "write_stdin": {
    "properties": [
      "chars",
      "max_output_tokens",
      "session_id",
      "yield_time_ms"
    ],
    "required": [
      "session_id"
    ]
  }
}"#
    );
    for old_name in [
        "exec_command",
        "read_file",
        "list_directory",
        "search_files",
        "stat_path",
    ] {
        assert!(
            !service
                .tool_router
                .list_all()
                .iter()
                .any(|tool| tool.name == old_name),
            "legacy MCP tool remains registered: {old_name}"
        );
    }
}

#[tokio::test]
async fn every_input_property_has_a_description() {
    let temp = tempdir().expect("tempdir");
    let service = DeskForemanService::new(app_state(temp.path().to_path_buf()));
    for tool in service.tool_router.list_all() {
        let Some(properties) = tool
            .input_schema
            .get("properties")
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (name, schema) in properties {
            assert!(
                schema.get("description").and_then(Value::as_str).is_some(),
                "tool {} property {} is missing a description",
                tool.name,
                name
            );
        }
    }
}

#[test]
fn shell_defaults_match_runtime_behavior() {
    let params: ShellParams = parse_params(json!({ "command": "pwd" })).expect("params");
    assert!(params.timeout.is_none());
    assert!(params.workdir.is_none());
    assert!(params.max_output_tokens.is_none());
}

#[test]
fn shell_rejects_host_only_fields() {
    let error = parse_params::<ShellParams>(json!({
        "command": "pwd",
        "sandbox_permissions": "use_default"
    }))
    .expect_err("unsupported field should fail");
    assert!(error.to_string().contains("sandbox_permissions"));
}

#[test]
fn openapi_includes_http_tool_paths() {
    let document = openapi_document();
    let paths = document.paths.paths;

    for path in [
        "/api/tools/shell",
        "/api/tools/write-stdin",
        "/api/tools/apply-patch",
        "/api/tools/read",
        "/api/tools/glob",
        "/api/tools/grep",
        "/api/tools/stat",
        "/api/admin/users/{user_id}/tools/shell",
        "/api/admin/users/{user_id}/tools/write-stdin",
        "/api/admin/users/{user_id}/tools/apply-patch",
        "/api/admin/users/{user_id}/tools/read",
        "/api/admin/users/{user_id}/tools/glob",
        "/api/admin/users/{user_id}/tools/grep",
        "/api/admin/users/{user_id}/tools/stat",
        "/api/admin/approval-settings",
        "/api/admin/approval-settings/test",
        "/api/admin/applications/{application_id}/approval-test",
    ] {
        assert!(paths.contains_key(path), "missing OpenAPI path {path}");
    }
    for path in [
        "/api/tools/exec-command",
        "/api/tools/read-file",
        "/api/tools/list-directory",
        "/api/tools/search-files",
        "/api/tools/stat-path",
    ] {
        assert!(
            !paths.contains_key(path),
            "legacy OpenAPI path remains: {path}"
        );
    }

    assert!(
        document
            .components
            .as_ref()
            .expect("OpenAPI components should be present")
            .schemas
            .contains_key("ApprovalSettingsResponse")
    );
    assert!(
        document
            .components
            .as_ref()
            .expect("OpenAPI components should be present")
            .schemas
            .contains_key("UpdateApprovalSettingsRequest")
    );
    assert!(
        document
            .components
            .as_ref()
            .expect("OpenAPI components should be present")
            .schemas
            .contains_key("ApprovalTestResponse")
    );
}
