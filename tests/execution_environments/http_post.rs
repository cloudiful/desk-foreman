//! Minimal HTTP/1.1 POST helper without extra client dependencies.

/// Minimal HTTP/1.1 POST helper without extra client dependencies.
pub async fn http_post(
    base: &str,
    path: &str,
    token: Option<&str>,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    crate::http_client::raw_request(base, "POST", path, token, Some(body)).await
}
