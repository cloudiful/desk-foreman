//! Raw HTTP caller for the native-task suite (issue 753 P1).
//!
//! Split from `support.rs` so each file stays inside the size budget. The
//! caller keeps the response head so a test can read a status code, a
//! Set-Cookie header and a chunk-decoded body without pulling in an HTTP
//! client dependency.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A raw HTTP response, kept as bytes so binary bodies survive.
#[derive(Clone, Debug)]
pub struct HttpOutcome {
    pub status: u16,
    pub body: Vec<u8>,
    /// The `Set-Cookie` value the server returned, when it returned one.
    pub set_cookie: Option<String>,
}

impl HttpOutcome {
    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// Split an `http://host:port` base into its parts.
pub fn split_base(base: &str) -> (String, u16) {
    let without_scheme = base.strip_prefix("http://").expect("http base URL");
    let (host, port) = without_scheme.rsplit_once(':').expect("host:port");
    (host.to_string(), port.parse().expect("port"))
}

/// Send one request with explicit method, path, headers and body.
pub async fn call(
    base: &str,
    method: &str,
    path: &str,
    token: Option<&str>,
    headers: &[(String, String)],
    body: &[u8],
) -> HttpOutcome {
    let (host, port) = split_base(base);
    let mut stream = tokio::net::TcpStream::connect(format!("{host}:{port}"))
        .await
        .expect("connect test server");
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\
         Content-Length: {}\r\n",
        body.len()
    );
    if !body.is_empty() {
        request.push_str("Content-Type: application/json\r\n");
    }
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
        request.push_str("X-DF-External-User-Id: integration-test-user\r\n");
    }
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    // A request refused from its declared length is answered and closed while
    // the client is still writing, so a write error here is expected and does
    // not prevent reading the response.
    let _ = stream.write_all(request.as_bytes()).await;
    if !body.is_empty() {
        let _ = stream.write_all(body).await;
    }
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), stream.read_to_end(&mut raw))
        .await
        .expect("task API must not hang")
        .expect("read response");
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
        .unwrap_or(raw.len());
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let status: u16 = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("status code");
    let body = raw[split..].to_vec();
    let chunked = head.lines().skip(1).any(|line| {
        line.to_ascii_lowercase().starts_with("transfer-encoding:")
            && line.to_ascii_lowercase().contains("chunked")
    });
    let set_cookie = head
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("set-cookie:"))
        .map(|line| line["set-cookie:".len()..].trim().to_string());
    HttpOutcome {
        status,
        body: if chunked { decode_chunked(&body) } else { body },
        set_cookie,
    }
}

/// Decode a chunked transfer body so an assertion sees the JSON, not framing.
fn decode_chunked(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rest = raw;
    loop {
        let Some(line_end) = rest.windows(2).position(|window| window == b"\r\n") else {
            break;
        };
        let size = match std::str::from_utf8(&rest[..line_end])
            .ok()
            .and_then(|line| {
                usize::from_str_radix(line.split(';').next().unwrap_or("").trim(), 16).ok()
            }) {
            Some(size) => size,
            None => break,
        };
        let start = line_end + 2;
        if size == 0 {
            break;
        }
        let end = (start + size).min(rest.len());
        out.extend_from_slice(&rest[start..end]);
        rest = &rest[(end + 2).min(rest.len())..];
    }
    out
}
