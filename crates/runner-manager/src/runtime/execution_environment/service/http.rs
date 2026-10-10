//! Private agent HTTP for one admitted stream (issue 749 P2c).
//!
//! Sends exactly what admission approved to the derived private address and
//! streams the answer back without ever buffering a whole response. The
//! client never redirects and never consults environment proxies: either
//! would let a response or a credential leave the trusted network. A
//! cancelled stream stops waiting and reports itself cancelled rather than
//! synthesizing a success.

use std::time::Duration;

use futures_util::{Stream, StreamExt};
use reqwest::redirect::Policy;
use tokio::sync::watch;

use super::relay::AdmittedService;

/// Deadline for the agent's response head.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest one stream may stay open.
const STREAM_TIMEOUT: Duration = Duration::from_secs(3600);
/// Largest response body accepted for one stream.
const MAX_RESPONSE_BYTES: u64 = 128 * 1024 * 1024;
/// Largest single forwarded header value.
const MAX_HEADER_VALUE_BYTES: usize = 2 * 1024;

/// Bounds for one agent hop.
///
/// Production uses the fixed constants above; tests inject small limits
/// through [`send_to_agent_with_limits`] so timeout and overflow paths run
/// in milliseconds without weakening the production bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HttpLimits {
    pub(crate) head: Duration,
    pub(crate) stream: Duration,
    pub(crate) max_bytes: u64,
}

/// The fixed production bounds. Tests never mutate these; they pass their
/// own [`HttpLimits`] to the `_with_limits` variant instead.
pub(crate) const PROD_LIMITS: HttpLimits = HttpLimits {
    head: HEAD_TIMEOUT,
    stream: STREAM_TIMEOUT,
    max_bytes: MAX_RESPONSE_BYTES,
};

/// Response head plus a bounded, cancellable body stream.
pub(crate) struct AgentReply {
    pub(crate) status: u16,
    pub(crate) headers: Vec<String>,
    pub(crate) body: AgentBody,
}

/// Body stream yielding one agent chunk at a time.
pub(crate) struct AgentBody {
    stream: std::pin::Pin<Box<dyn Stream<Item = std::io::Result<Vec<u8>>> + Send>>,
}

impl AgentBody {
    pub(crate) async fn next(&mut self) -> Option<std::io::Result<Vec<u8>>> {
        self.stream.next().await
    }
}

/// Why the agent hop failed. Fixed variants only: no upstream diagnostic
/// text is ever echoed, and the token never appears in any message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentError {
    /// The stream was cancelled (stop, tunnel loss, or client drop).
    Cancelled,
    /// The agent answered 404 for the relayed resource.
    NotFound,
    /// The agent did not answer in time.
    Timeout,
    /// The tunnel or agent is unreachable, or the response is unusable.
    Unavailable,
}

/// Send one assembled request to the admitted private target.
///
/// `body` must already be the exact declared bytes; this function never
/// reframes or re-lengths it. Only `content-type` and `accept` cross to the
/// agent; the per-environment credential travels as Basic auth derived from
/// the trusted username and the stored token.
pub(crate) async fn send_to_agent(
    admitted: &AdmittedService,
    method: &str,
    path: &str,
    query: Option<&str>,
    allowed_headers: &[String],
    body: Vec<u8>,
    cancel: &watch::Receiver<bool>,
) -> Result<AgentReply, AgentError> {
    send_to_agent_with_limits(
        admitted,
        method,
        path,
        query,
        allowed_headers,
        body,
        cancel,
        &PROD_LIMITS,
    )
    .await
}

/// [`send_to_agent`] with caller-supplied bounds.
///
/// Test-only entry in practice: fast timeout and overflow tests inject tiny
/// limits here while production always passes [`PROD_LIMITS`].
pub(crate) async fn send_to_agent_with_limits(
    admitted: &AdmittedService,
    method: &str,
    path: &str,
    query: Option<&str>,
    allowed_headers: &[String],
    body: Vec<u8>,
    cancel: &watch::Receiver<bool>,
    limits: &HttpLimits,
) -> Result<AgentReply, AgentError> {
    // Owned copies: the streamed body outlives this call, so the async
    // body must not borrow the caller's limits.
    let HttpLimits {
        head: head_timeout,
        stream: stream_timeout,
        max_bytes,
    } = *limits;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| AgentError::Unavailable)?;
    // Compose the authority through `SocketAddr`, which brackets an IPv6
    // literal: interpolating a bare `IpAddr` would yield `http://::1:8443`,
    // which is not a URL. `SocketAddr::new` is infallible for any address
    // the private-target validator can produce, and it cannot introduce a
    // scheme, path, or host of its own — the target is still only the
    // inspected private address plus the trusted profile port.
    let authority = std::net::SocketAddr::new(admitted.target, admitted.port);
    let mut url = format!("http://{authority}/")
        .parse::<reqwest::Url>()
        .map_err(|_| AgentError::Unavailable)?;
    url.set_path(&format!("/{path}"));
    if let Some(query) = query {
        url.set_query(Some(query));
    }
    let request_method = method
        .parse::<reqwest::Method>()
        .map_err(|_| AgentError::Unavailable)?;
    let mut request = client
        .request(request_method, url)
        .basic_auth(&admitted.username, Some(&admitted.token));
    for (name, value) in permitted_agent_headers(allowed_headers) {
        request = request.header(name, value);
    }
    let request = request
        .body(body)
        .build()
        .map_err(|_| AgentError::Unavailable)?;

    if *cancel.borrow() {
        return Err(AgentError::Cancelled);
    }
    let response = tokio::time::timeout(head_timeout, client.execute(request))
        .await
        .map_err(|_| AgentError::Timeout)?
        .map_err(|_| AgentError::Unavailable)?;
    if *cancel.borrow() {
        return Err(AgentError::Cancelled);
    }
    let status = response.status().as_u16();
    if status == 404 {
        return Err(AgentError::NotFound);
    }
    let headers = permitted_reply_headers(response.headers());
    Ok(AgentReply {
        status,
        headers,
        body: AgentBody {
            stream: Box::pin(futures_util::stream::unfold(
                super::http_body::BodyCursor {
                    stream: Box::pin(response.bytes_stream().map(|result| {
                        result
                            .map(|bytes| bytes.to_vec())
                            .map_err(|_| super::http_body::unavailable_io())
                    })),
                    received: 0,
                    cancel: cancel.clone(),
                    deadline: tokio::time::Instant::now() + stream_timeout,
                    max_bytes,
                },
                super::http_body::next_body_chunk,
            )),
        },
    })
}

/// Only `content-type` and `accept` may cross to the agent, parsed back
/// from the `name: value` entries the control plane admitted.
fn permitted_agent_headers(entries: &[String]) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    for entry in entries {
        let Some((name, value)) = entry.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        if name != "content-type" && name != "accept" {
            continue;
        }
        if value.len() > MAX_HEADER_VALUE_BYTES
            || value.chars().any(|character| character.is_control())
            || value.is_empty()
        {
            continue;
        }
        headers.push((name, value.to_string()));
    }
    headers
}

/// Only the reply headers the control plane permits travel back, so a
/// relayed response can never set a cookie or redirect its caller even
/// before the gateway filters it again.
fn permitted_reply_headers(headers: &reqwest::header::HeaderMap) -> Vec<String> {
    let mut permitted = Vec::new();
    for name in ["content-type", "cache-control", "retry-after"] {
        let Some(value) = headers.get(name).and_then(|value| value.to_str().ok()) else {
            continue;
        };
        if value.len() > MAX_HEADER_VALUE_BYTES
            || value.chars().any(|character| character.is_control())
            || value.is_empty()
        {
            continue;
        }
        permitted.push(format!("{name}: {value}"));
    }
    permitted
}
