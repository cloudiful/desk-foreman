use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::{Validate, ValidationError};

#[derive(utoipa::IntoParams)]
#[into_params(parameter_in = Header)]
pub struct IdentityHeaders {
    #[param(rename = "X-DF-External-User-Id")]
    pub external_user_id: String,
    #[param(rename = "X-DF-Workspace-Key")]
    pub workspace_key: String,
    #[param(rename = "X-DF-Lease-Owner")]
    pub lease_owner: Option<String>,
}

fn nonblank(value: &str) -> Result<(), ValidationError> {
    if value.trim().is_empty() {
        Err(ValidationError::new("empty"))
    } else {
        Ok(())
    }
}

#[derive(Debug, Deserialize, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
pub struct StartRequest {
    #[validate(length(min = 1, max = 32768), custom(function = "nonblank"))]
    /// Nonblank command, at most 32768 UTF-8 bytes.
    #[schema(min_length = 1, max_length = 32768)]
    pub command: String,
    pub workdir: Option<String>,
    #[validate(range(min = 1, max = 600000))]
    #[schema(minimum = 1, maximum = 600000)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
pub struct InputRequest {
    #[validate(length(min = 1, max = 128))]
    #[schema(min_length = 1, max_length = 128)]
    pub execution_id: String,
    #[validate(length(min = 1, max = 65536))]
    /// Nonempty input, at most 65536 UTF-8 bytes.
    #[schema(min_length = 1, max_length = 65536)]
    pub chars: String,
}

#[derive(Debug, Deserialize, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
pub struct OutputQuery {
    #[validate(length(min = 1, max = 128))]
    pub cursor: Option<String>,
    #[validate(range(min = 1, max = 1048576))]
    pub max_bytes: Option<usize>,
}

#[derive(Serialize, ToSchema)]
pub struct StartResponse {
    pub execution: crate::tools::execution::Descriptor,
    pub output: crate::tools::execution::Output,
}

#[derive(Serialize, ToSchema)]
pub struct StartFailure {
    pub error: String,
    pub execution_id: String,
    pub state: String,
}

#[derive(ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StartGenericFailure {
    pub error: String,
}

#[derive(ToSchema)]
#[serde(untagged)]
pub enum StartServiceUnavailable {
    Generic(StartGenericFailure),
    Uncertain(StartFailure),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_tools_dto_shell_unavailable_schema_has_both_shapes() {
        let doc = serde_json::to_value(crate::api::openapi_document()).unwrap();
        let schemas = &doc["components"]["schemas"];
        let response = &doc["paths"]["/api/internal/workspace-tools/shell"]["post"]["responses"]["503"]
            ["content"]["application/json"]["schema"];
        assert_eq!(
            response["$ref"],
            "#/components/schemas/StartServiceUnavailable"
        );
        let variants = schemas["StartServiceUnavailable"]["oneOf"]
            .as_array()
            .unwrap();
        for name in ["StartGenericFailure", "StartFailure"] {
            assert!(
                variants
                    .iter()
                    .any(|v| v["$ref"] == format!("#/components/schemas/{name}"))
            );
        }
        let generic = schemas["StartGenericFailure"]["required"]
            .as_array()
            .unwrap();
        let uncertain = schemas["StartFailure"]["required"].as_array().unwrap();
        assert_eq!(
            schemas["StartGenericFailure"]["additionalProperties"],
            false
        );
        assert!(!generic.iter().any(|v| v == "execution_id"));
        assert!(uncertain.iter().any(|v| v == "execution_id"));
    }

    #[test]
    fn workspace_tools_dto_schema_carries_limits_and_headers() {
        let document = serde_json::to_value(crate::api::openapi_document()).unwrap();
        let schema = &document["components"]["schemas"]["StartRequest"];
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["command"]["maxLength"], 32768);
        assert_eq!(schema["properties"]["timeout_ms"]["minimum"], 1);
        let read = &document["components"]["schemas"]["ReadParams"]["properties"];
        assert_eq!(read["offset"]["minimum"], 1);
        assert_eq!(read["limit"]["minimum"], 1);
        assert_eq!(read["limit"]["maximum"], 2000);
        let paths = document["paths"].as_object().unwrap();
        let operations: Vec<_> = paths
            .iter()
            .filter(|(path, _)| path.starts_with("/api/internal/workspace-tools/"))
            .flat_map(|(_, item)| {
                item.as_object()
                    .unwrap()
                    .iter()
                    .filter(|(method, _)| matches!(method.as_str(), "get" | "post"))
                    .map(|(_, op)| op)
            })
            .collect();
        assert_eq!(operations.len(), 11);
        for op in operations {
            for status in ["200", "401", "403", "503"] {
                assert!(
                    op["responses"].get(status).is_some(),
                    "{} missing {status}",
                    op["operationId"]
                );
            }
            assert_eq!(
                op["security"],
                serde_json::json!([{"application_bearer": []}])
            );
            for name in ["X-DF-External-User-Id", "X-DF-Workspace-Key"] {
                assert!(op["parameters"].as_array().unwrap().iter().any(|parameter| parameter["name"] == name && parameter["required"] == true));
            }
        }
    }

    #[test]
    fn workspace_tools_dto_reject_unknown_and_invalid_values() {
        assert!(serde_json::from_str::<StartRequest>(r#"{"command":"pwd","tty":true}"#).is_err());
        assert!(
            serde_json::from_str::<InputRequest>(r#"{"execution_id":"a","chars":""}"#)
                .unwrap()
                .validate()
                .is_err()
        );
        assert!(
            serde_json::from_str::<StartRequest>(r#"{"command":" ","timeout_ms":0}"#)
                .unwrap()
                .validate()
                .is_err()
        );
        assert!(
            serde_json::from_str::<OutputQuery>(r#"{"max_bytes":1048577}"#)
                .unwrap()
                .validate()
                .is_err()
        );
    }
}
