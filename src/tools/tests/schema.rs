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
async fn exported_catalog_matches_serving_registry() {
    let temp = tempdir().expect("tempdir");
    let service = DeskForemanService::new(app_state(temp.path().to_path_buf()));
    let mut registered = service.tool_router.list_all();
    registered.sort_by(|a, b| a.name.cmp(&b.name));
    let catalog = DeskForemanService::tool_catalog();
    assert!(!catalog.is_empty());
    assert!(catalog.windows(2).all(|pair| pair[0].name < pair[1].name));
    assert!(catalog.iter().all(|tool| !tool.input_schema.is_empty()));
    assert_eq!(
        serde_json::to_value(&catalog).unwrap(),
        serde_json::to_value(&registered).unwrap()
    );
    let exported: Value = serde_json::from_str(include_str!(
        "../../../contracts/mcp/desk-foreman-tool-catalog.json"
    ))
    .unwrap();
    assert_eq!(exported, json!({"schema_version": 1, "tools": registered}));
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

#[tokio::test]
async fn apply_patch_output_schema_is_a_strict_three_variant_union() {
    let temp = tempdir().expect("tempdir");
    let service = DeskForemanService::new(app_state(temp.path().to_path_buf()));
    let apply_patch = service
        .tool_router
        .list_all()
        .into_iter()
        .find(|tool| tool.name == "apply_patch")
        .expect("apply_patch registered");
    let schema = apply_patch
        .output_schema
        .expect("apply_patch output schema");
    let variants = schema
        .get("oneOf")
        .and_then(Value::as_array)
        .expect("strict result union");
    assert_eq!(variants.len(), 3);
    for variant in variants {
        assert_eq!(
            variant.get("additionalProperties").and_then(Value::as_bool),
            Some(false)
        );
    }
    let outcomes = variants
        .iter()
        .filter_map(|variant| {
            variant["properties"]["outcome"]["const"]
                .as_str()
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    assert_eq!(outcomes, ["applied", "rejected", "incomplete"]);

    let openapi = openapi_document();
    let schemas = openapi.components.expect("components").schemas;
    for name in [
        "PatchResultOutput",
        "PatchAppliedOutput",
        "PatchRejectedOutput",
        "PatchIncompleteOutput",
        "PatchChangeOutput",
        "PatchDiagnosticOutput",
        "PatchChangeErrorOutput",
    ] {
        assert!(schemas.contains_key(name), "missing OpenAPI schema {name}");
    }
}

#[tokio::test]
async fn apply_patch_output_schema_requires_all_nullable_fields() {
    let temp = tempdir().expect("tempdir");
    let service = DeskForemanService::new(app_state(temp.path().to_path_buf()));
    let apply_patch = service
        .tool_router
        .list_all()
        .into_iter()
        .find(|tool| tool.name == "apply_patch")
        .expect("apply_patch registered");
    let schema = apply_patch
        .output_schema
        .expect("apply_patch output schema");
    let defs = schema
        .get("$defs")
        .and_then(Value::as_object)
        .expect("defs");
    let change = &defs["PatchChangeOutput"];
    let diagnostic = &defs["PatchDiagnosticOutput"];

    assert_eq!(
        required_names(change),
        string_set(&[
            "operation",
            "path",
            "destination",
            "status",
            "effect",
            "error"
        ])
    );
    assert_eq!(
        required_names(diagnostic),
        string_set(&["code", "stage", "path", "operation", "hunk", "message"])
    );
    for (label, definition, fields) in [
        ("change", change, &["destination", "error"][..]),
        ("diagnostic", diagnostic, &["path", "operation", "hunk"][..]),
    ] {
        assert_eq!(
            definition
                .get("additionalProperties")
                .and_then(Value::as_bool),
            Some(false),
            "{label} stays closed"
        );
        for field in fields {
            assert!(
                allows_null(&definition["properties"][field]),
                "{label}.{field} must still admit null"
            );
        }
    }

    let variants = schema["oneOf"].as_array().expect("variants");
    let applied = variants
        .iter()
        .find(|variant| variant["properties"]["outcome"]["const"] == "applied")
        .expect("applied variant");
    assert_eq!(applied["properties"]["changes"]["minItems"], 1);
    let incomplete = variants
        .iter()
        .find(|variant| variant["properties"]["outcome"]["const"] == "incomplete")
        .expect("incomplete variant");
    assert!(
        incomplete["properties"]["changes"]
            .get("minItems")
            .is_none(),
        "incomplete changes must not gain a minimum"
    );
}

#[test]
fn openapi_patch_schemas_require_all_nullable_fields() {
    let document = openapi_document();
    let value = serde_json::to_value(&document).expect("openapi serializes");
    let schemas = &value["components"]["schemas"];
    let change = &schemas["PatchChangeOutput"];
    let diagnostic = &schemas["PatchDiagnosticOutput"];

    assert_eq!(
        required_names(change),
        string_set(&[
            "operation",
            "path",
            "destination",
            "status",
            "effect",
            "error"
        ])
    );
    assert_eq!(
        required_names(diagnostic),
        string_set(&["code", "stage", "path", "operation", "hunk", "message"])
    );
    for (label, definition, fields) in [
        ("change", change, &["destination", "error"][..]),
        ("diagnostic", diagnostic, &["path", "operation", "hunk"][..]),
    ] {
        assert_eq!(
            definition
                .get("additionalProperties")
                .and_then(Value::as_bool),
            Some(false),
            "{label} stays closed"
        );
        for field in fields {
            assert!(
                allows_null(&definition["properties"][field]),
                "{label}.{field} must still admit null"
            );
        }
    }
    assert_eq!(
        schemas["PatchAppliedOutput"]["properties"]["changes"]["minItems"],
        1
    );
    assert!(
        schemas["PatchIncompleteOutput"]["properties"]["changes"]
            .get("minItems")
            .is_none(),
        "incomplete changes must not gain a minimum"
    );
}

fn required_names(schema: &Value) -> std::collections::BTreeSet<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn string_set(values: &[&str]) -> std::collections::BTreeSet<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

fn allows_null(schema: &Value) -> bool {
    if schema.get("type").and_then(Value::as_str) == Some("null") {
        return true;
    }
    if schema
        .get("type")
        .and_then(Value::as_array)
        .is_some_and(|types| types.iter().any(|value| value == "null"))
    {
        return true;
    }
    ["anyOf", "oneOf", "allOf"].iter().any(|keyword| {
        schema
            .get(*keyword)
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().any(allows_null))
    })
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
}
