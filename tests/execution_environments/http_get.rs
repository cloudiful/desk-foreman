//! Minimal HTTP/1.1 GET helper without extra client dependencies.

/// Minimal HTTP/1.1 GET helper without extra client dependencies.
pub async fn http_get(base: &str, path: &str, token: Option<&str>) -> (u16, serde_json::Value) {
    crate::http_client::raw_request(base, "GET", path, token, None).await
}
