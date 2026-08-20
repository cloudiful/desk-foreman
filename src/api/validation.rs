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
            .map_err(|rejection| map_query_rejection(parts, rejection))?;
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

pub fn validate_lifecycle_state(value: &str) -> Result<(), ValidationError> {
    if !matches!(value, "active" | "archived" | "resetting") {
        return Err(validation_error(
            "must be one of: active, archived, resetting",
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

fn map_query_rejection(parts: &Parts, rejection: QueryRejection) -> AppError {
    let message = rejection.body_text();
    tracing::warn!(
        method = %parts.method,
        path = %parts.uri.path(),
        rejection = %message,
        "rejected malformed query string"
    );
    AppError::bad_request(message)
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
    use std::{
        io,
        sync::{Arc, Mutex},
    };

    use serde::Deserialize;

    use super::{
        deserialize_optional_trimmed_nonempty, map_query_rejection, validate_non_blank,
        validate_sort_dir, validate_user_sort_by,
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

    #[test]
    fn query_rejection_logs_method_and_path_without_query() {
        // Forces a QueryRejection by deserializing an unknown field as the wrong type.
        #[derive(Deserialize, Debug)]
        #[allow(dead_code)]
        struct StrictParams {
            limit: u32,
        }

        let raw_query = "limit=not-a-number&secret=topsecret&token=abc123";
        let uri: axum::http::Uri = format!("/api/admin/applications?{raw_query}")
            .parse()
            .expect("uri");
        let request = axum::http::Request::builder()
            .method("GET")
            .uri(uri.clone())
            .body(())
            .expect("request");
        let (parts, _) = request.into_parts();

        let rejection = axum::extract::Query::<StrictParams>::try_from_uri(&uri)
            .err()
            .expect("malformed limit should be rejected");

        // Build a subscriber that writes to an in-memory buffer so we can assert
        // the warn log includes method/path but never the query string or its values.
        let buffer = Arc::new(Mutex::new(Vec::<u8>::new()));
        let writer = SharedWriter(Arc::clone(&buffer));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer)
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .with_target(false)
            .finish();

        let error = tracing::subscriber::with_default(subscriber, || {
            map_query_rejection(&parts, rejection)
        });

        let captured = String::from_utf8(buffer.lock().expect("buffer lock").clone())
            .expect("utf8 log output");

        assert!(
            captured.contains("WARN"),
            "warn level missing from log output: {captured}"
        );
        assert!(
            captured.contains("rejected malformed query string"),
            "log message missing: {captured}"
        );
        assert!(
            captured.contains("method=GET"),
            "method missing from log output: {captured}"
        );
        assert!(
            captured.contains("path=/api/admin/applications"),
            "path missing from log output: {captured}"
        );
        // Sensitive raw query values must never appear in the log payload.
        for needle in ["topsecret", "abc123", "secret=", "token=", raw_query] {
            assert!(
                !captured.contains(needle),
                "log output leaked query value ({needle}): {captured}"
            );
        }

        // BadRequest shape and body text are preserved.
        match error {
            crate::error::AppError::BadRequest(message) => {
                assert!(
                    !message.is_empty(),
                    "rejection body text should be propagated"
                );
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl io::Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().expect("buffer lock").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedWriter {
        type Writer = SharedWriter;

        fn make_writer(&'a self) -> Self::Writer {
            SharedWriter(Arc::clone(&self.0))
        }
    }
}
