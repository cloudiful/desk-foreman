//! Minimal HTTP/1.1 test client without extra dependencies.
//!
//! Uses `tokio::net::TcpStream` directly so the integration suite needs
//! no new client crate. `X-DF-External-User-Id` is sent on every
//! authenticated call because `application_actor` requires it, mirroring
//! the lease routes; environment identity still comes from the body.

fn parse_base(base: &str) -> (String, u16) {
    let without_scheme = base.strip_prefix("http://").expect("http base URL");
    let (host, port) = without_scheme.rsplit_once(':').expect("host:port");
    (host.to_string(), port.parse().expect("port"))
}

pub(super) async fn raw_request(
    base: &str,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (host, port) = parse_base(base);
    let mut stream = tokio::net::TcpStream::connect(format!("{host}:{port}"))
        .await
        .expect("connect test server");
    let body_text = body.map(|v| v.to_string()).unwrap_or_default();
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nConnection: close\r\n"
    );
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
        request.push_str("X-DF-External-User-Id: integration-test-user\r\n");
    }
    request.push_str(&format!("Content-Length: {}\r\n\r\n", body_text.len()));
    request.push_str(&body_text);
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read response");
    let text = String::from_utf8_lossy(&raw).to_string();
    let status: u16 = text
        .lines()
        .next()
        .expect("status line")
        .split_whitespace()
        .nth(1)
        .expect("status code")
        .parse()
        .expect("status number");
    let body_start = text.find("\r\n\r\n").map(|i| i + 4).unwrap_or(text.len());
    let body_text = text[body_start..].trim();
    let json = if body_text.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(body_text).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}
