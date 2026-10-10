//! OpenAPI tests for the application bearer security contract.
//!
//! The internal execution-environment routes already reject requests without
//! an application bearer token at runtime. These tests pin the generated
//! document to that same contract so downstream clients send the header, and
//! they fail if any requirement, the scheme itself, or a public route changes.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use super::{APPLICATION_BEARER_SCHEME, openapi_document};

const HTTP_METHODS: [&str; 7] = ["get", "post", "put", "patch", "delete", "head", "options"];

/// Operations guarded by `application_actor`, as `(path, method, operationId)`.
const APPLICATION_BEARER_OPERATIONS: [(&str, &str, &str); 13] = [
    (
        "/api/internal/execution-environments/ensure",
        "post",
        "ensure_execution_environment",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/invalidate",
        "post",
        "invalidate_execution_environment",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/provision",
        "post",
        "provision_execution_environment",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/stop",
        "post",
        "stop_execution_environment",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/replace",
        "post",
        "replace_execution_environment",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/status",
        "get",
        "execution_environment_status",
    ),
    // The private-service gateway resolves the same application identity from
    // the same bearer guard, so every relayed method is guarded by it too.
    (
        "/api/internal/execution-environments/{environment_id}/service/{path}",
        "get",
        "execution_environment_service_get",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/service/{path}",
        "post",
        "execution_environment_service_post",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/service/{path}",
        "put",
        "execution_environment_service_put",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/service/{path}",
        "patch",
        "execution_environment_service_patch",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/service/{path}",
        "delete",
        "execution_environment_service_delete",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/service/{path}",
        "head",
        "execution_environment_service_head",
    ),
    (
        "/api/internal/execution-environments/{environment_id}/service/{path}",
        "options",
        "execution_environment_service_options",
    ),
];

fn document() -> Value {
    serde_json::to_value(openapi_document()).expect("serialize openapi document")
}

/// Every exported operation keyed by `(path, method)`.
fn operations(document: &Value) -> BTreeMap<(&str, &str), &Value> {
    let mut found = BTreeMap::new();
    let paths = document["paths"].as_object().expect("paths object");
    for (path, item) in paths {
        let item = item.as_object().expect("path item object");
        for (method, operation) in item {
            if !HTTP_METHODS.contains(&method.as_str()) {
                continue;
            }
            found.insert((path.as_str(), method.as_str()), operation);
        }
    }
    found
}

/// The single unnamed security requirement expected on a bearer operation.
fn application_bearer_requirement() -> Value {
    let mut requirement = Map::new();
    requirement.insert(
        APPLICATION_BEARER_SCHEME.to_string(),
        Value::Array(Vec::new()),
    );
    Value::Array(vec![Value::Object(requirement)])
}

#[test]
fn application_bearer_is_declared_as_http_bearer_without_credentials() {
    let document = document();
    let schemes = document["components"]
        .as_object()
        .and_then(|components| components.get("securitySchemes"))
        .and_then(Value::as_object)
        .expect("components.securitySchemes object");
    let scheme = schemes
        .get(APPLICATION_BEARER_SCHEME)
        .unwrap_or_else(|| panic!("missing {APPLICATION_BEARER_SCHEME} security scheme"));

    assert_eq!(scheme["type"], json!("http"));
    assert_eq!(scheme["scheme"], json!("bearer"));
    assert!(
        scheme.get("bearerFormat").is_none(),
        "the application token is opaque, the scheme must not claim a bearer format"
    );
    for forbidden in ["value", "default", "example"] {
        assert!(
            scheme.get(forbidden).is_none(),
            "security scheme must not carry a {forbidden}"
        );
    }
}

#[test]
fn environment_operations_require_the_application_bearer() {
    let document = document();
    let operations = operations(&document);

    for (path, method, operation_id) in APPLICATION_BEARER_OPERATIONS {
        let operation = operations
            .get(&(path, method))
            .unwrap_or_else(|| panic!("missing operation {method} {path}"));
        assert_eq!(
            operation["operationId"],
            json!(operation_id),
            "{path} must keep its documented operationId"
        );
        assert_eq!(
            operation["security"],
            application_bearer_requirement(),
            "{operation_id} must require the application bearer scheme"
        );
        assert!(
            operation["responses"].get("401").is_some(),
            "{operation_id} must keep documenting its unauthorized response"
        );
    }
}

#[test]
fn only_the_environment_operations_declare_security() {
    let document = document();
    let mut guarded: Vec<(&str, &str, &str)> = operations(&document)
        .into_iter()
        .filter(|(_, operation)| operation.get("security").is_some())
        .map(|((path, method), operation)| {
            (
                path,
                method,
                operation["operationId"].as_str().expect("operationId"),
            )
        })
        .collect();
    guarded.sort_unstable();

    let mut expected: Vec<(&str, &str, &str)> = APPLICATION_BEARER_OPERATIONS.to_vec();
    expected.sort_unstable();

    assert_eq!(
        guarded, expected,
        "the bearer requirement must not spread to or vanish from other routes"
    );
}

#[test]
fn no_global_security_is_declared() {
    let document = document();
    assert!(
        document.get("security").is_none(),
        "global security would falsely apply to public endpoints"
    );
}

#[test]
fn public_operations_stay_unauthenticated() {
    let document = document();
    let operations = operations(&document);
    let login = operations
        .get(&("/api/auth/login", "post"))
        .expect("login operation");
    assert_eq!(login["operationId"], json!("login"));
    assert!(
        login.get("security").is_none(),
        "the public login operation must not require a security scheme"
    );

    if let Some(health) = document["paths"]
        .as_object()
        .and_then(|paths| paths.get("/healthz"))
    {
        for method in HTTP_METHODS {
            if let Some(operation) = health.get(method) {
                assert!(
                    operation.get("security").is_none(),
                    "the public /healthz {method} operation must not require a security scheme"
                );
            }
        }
    }
}

#[test]
fn every_security_requirement_references_a_declared_scheme() {
    let document = document();
    let declared: BTreeSet<&str> = document["components"]
        .get("securitySchemes")
        .and_then(Value::as_object)
        .expect("components.securitySchemes object")
        .keys()
        .map(String::as_str)
        .collect();

    for ((path, method), operation) in operations(&document) {
        let requirements = operation
            .get("security")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for requirement in requirements {
            let requirement = requirement
                .as_object()
                .expect("security requirement object");
            for scheme in requirement.keys() {
                assert!(
                    declared.contains(scheme.as_str()),
                    "{method} {path} requires undeclared security scheme {scheme}"
                );
            }
        }
    }
}

#[test]
fn openapi_export_is_byte_identical_across_builds() {
    let first = serde_json::to_string_pretty(&openapi_document()).expect("first export");
    let second = serde_json::to_string_pretty(&openapi_document()).expect("second export");
    assert_eq!(first, second, "the OpenAPI export must be byte-identical");
}
