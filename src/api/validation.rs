use std::borrow::Cow;

use axum::{
    Json,
    extract::{
        FromRequest, FromRequestParts, Query, Request,
        rejection::{JsonRejection, QueryRejection},
    },
    http::request::Parts,
};
use serde::Deserialize;
use validator::{Validate, ValidationError, ValidationErrors, ValidationErrorsKind};

use crate::error::AppError;

pub struct ValidatedJson<T>(pub T);

impl<S, T> FromRequest<S> for ValidatedJson<T>
where
    S: Send + Sync,
    Json<T>: FromRequest<S, Rejection = JsonRejection>,
    T: Validate,
{
    type Rejection = AppError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(req, state)
            .await
            .map_err(map_json_rejection)?;
        value.validate().map_err(map_validation_errors)?;
        Ok(Self(value))
    }
}

pub struct ValidatedQuery<T>(pub T);

impl<S, T> FromRequestParts<S> for ValidatedQuery<T>
where
    S: Send + Sync,
    Query<T>: FromRequestParts<S, Rejection = QueryRejection>,
    T: Validate,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Query(value) = Query::<T>::from_request_parts(parts, state)
            .await
            .map_err(map_query_rejection)?;
        value.validate().map_err(map_validation_errors)?;
        Ok(Self(value))
    }
}

pub fn validate_non_blank(value: &str) -> Result<(), ValidationError> {
    if value.trim().is_empty() {
        return Err(validation_error("must not be blank"));
    }
    Ok(())
}

pub fn validate_user_sort_by(value: &str) -> Result<(), ValidationError> {
    if !matches!(
        value,
        "login_name" | "display_name" | "created_at" | "updated_at" | "last_login_at"
    ) {
        return Err(validation_error(
            "must be one of: login_name, display_name, created_at, updated_at, last_login_at",
        ));
    }
    Ok(())
}

pub fn validate_sort_dir(value: &str) -> Result<(), ValidationError> {
    if !matches!(value, "asc" | "desc") {
        return Err(validation_error("must be one of: asc, desc"));
    }
    Ok(())
}

pub fn validate_audit_status(value: &str) -> Result<(), ValidationError> {
    if !matches!(value, "success" | "failure" | "unknown") {
        return Err(validation_error(
            "must be one of: success, failure, unknown",
        ));
    }
    Ok(())
}

pub fn validate_read_file_params<T>(value: &T) -> Result<(), ValidationError>
where
    T: ReadFileRangeValidation,
{
    if let (Some(start), Some(end)) = (value.start_line(), value.end_line())
        && end < start
    {
        return Err(validation_error(
            "end_line must be greater than or equal to start_line",
        ));
    }
    Ok(())
}

pub fn deserialize_optional_trimmed_nonempty<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    Ok(value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }))
}

fn map_validation_errors(errors: ValidationErrors) -> AppError {
    AppError::bad_request(validation_errors_message(&errors))
}

fn map_json_rejection(rejection: JsonRejection) -> AppError {
    AppError::bad_request(rejection.body_text())
}

fn map_query_rejection(rejection: QueryRejection) -> AppError {
    AppError::bad_request(rejection.body_text())
}

fn validation_error(message: &'static str) -> ValidationError {
    let mut error = ValidationError::new("validation");
    error.message = Some(Cow::Borrowed(message));
    error
}

pub fn validation_errors_message(errors: &ValidationErrors) -> String {
    let mut messages = Vec::new();
    collect_validation_errors(None, errors, &mut messages);
    if messages.is_empty() {
        "validation failed".to_string()
    } else {
        messages.join(", ")
    }
}

fn collect_validation_errors(
    prefix: Option<&str>,
    errors: &ValidationErrors,
    messages: &mut Vec<String>,
) {
    for (field, kind) in errors.errors() {
        let name = prefix
            .map(|prefix| format!("{prefix}.{field}"))
            .unwrap_or_else(|| field.to_string());
        match kind {
            ValidationErrorsKind::Field(field_errors) => {
                for error in field_errors {
                    let message = error
                        .message
                        .as_deref()
                        .map(str::to_string)
                        .unwrap_or_else(|| error.code.to_string());
                    messages.push(format!("{name}: {message}"));
                }
            }
            ValidationErrorsKind::Struct(nested) => {
                collect_validation_errors(Some(&name), nested, messages);
            }
            ValidationErrorsKind::List(items) => {
                for (index, nested) in items {
                    collect_validation_errors(Some(&format!("{name}[{index}]")), nested, messages);
                }
            }
        }
    }
}

pub trait ReadFileRangeValidation {
    fn start_line(&self) -> Option<usize>;
    fn end_line(&self) -> Option<usize>;
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::{
        deserialize_optional_trimmed_nonempty, validate_non_blank, validate_sort_dir,
        validate_user_sort_by,
    };

    #[test]
    fn non_blank_validator_rejects_whitespace() {
        assert!(validate_non_blank("   ").is_err());
        assert!(validate_non_blank("alice").is_ok());
    }

    #[test]
    fn sort_validators_reject_unsupported_values() {
        assert!(validate_user_sort_by("invalid").is_err());
        assert!(validate_sort_dir("sideways").is_err());
    }

    #[test]
    fn optional_deserializer_trims_and_drops_blank() {
        #[derive(Deserialize)]
        struct Input {
            #[serde(deserialize_with = "deserialize_optional_trimmed_nonempty")]
            value: Option<String>,
        }

        let blank: Input = serde_json::from_str(r#"{ "value": "   " }"#).expect("blank");
        assert_eq!(blank.value, None);

        let text: Input = serde_json::from_str(r#"{ "value": "  abc  " }"#).expect("text");
        assert_eq!(text.value.as_deref(), Some("abc"));
    }
}
