use crate::error::AppError;
use axum::http::HeaderMap;

pub(crate) const RUNNER_ISOLATION_HEADER: &str = "X-DF-Runner-Isolation";

pub(crate) fn requested(headers: &HeaderMap) -> Result<bool, AppError> {
    let mut values = headers.get_all(RUNNER_ISOLATION_HEADER).iter();
    let Some(value) = values.next() else {
        return Ok(false);
    };
    if values.next().is_some() || value.as_bytes() != b"principal" {
        return Err(AppError::unauthorized("invalid runner isolation header"));
    }
    Ok(true)
}

pub(crate) fn required_claim(headers: &HeaderMap, name: &str) -> Result<String, AppError> {
    let mut values = headers.get_all(name).iter();
    let value = values
        .next()
        .ok_or_else(|| AppError::unauthorized(format!("missing {name} header")))?;
    if values.next().is_some() {
        return Err(AppError::unauthorized(format!("duplicate {name} header")));
    }
    let value = value
        .to_str()
        .map_err(|_| AppError::unauthorized(format!("invalid {name} header")))?;
    if value.is_empty() || value.len() > 1024 {
        return Err(AppError::unauthorized(format!("invalid {name} header")));
    }
    Ok(value.to_string())
}

#[cfg(test)]
#[path = "runner_isolation_tests.rs"]
mod tests;
