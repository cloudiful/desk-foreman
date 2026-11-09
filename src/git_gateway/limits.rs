use axum::http::{HeaderMap, header};

pub(crate) fn declared_content_length(headers: &HeaderMap) -> Option<usize> {
    headers
        .get(header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse::<usize>()
        .ok()
}

pub(crate) fn exceeds_limit(declared: Option<usize>, max: usize) -> bool {
    declared.is_some_and(|length| length > max)
}
