//! Streaming one relayed response back to the caller (issue 749 P2b).
//!
//! The response is streamed, never accumulated: each agent chunk becomes one
//! body slice as it arrives, so a long-lived stream such as server-sent
//! events reaches the caller while the agent is still producing it, and a
//! caller that stops reading applies backpressure rather than filling memory.
//!
//! Only three response headers are copied back. A `Set-Cookie`, a `Location`
//! or anything else that could redirect or re-authenticate the caller is
//! dropped here, so a relayed response can never navigate the caller off the
//! agent or attach a new credential to it.

use std::io;

use axum::{
    body::Body,
    http::{HeaderName, HeaderValue, Response},
};
use runner_protocol::service_relay::{RelayErrorCode, ResponseHeadFrame};
use tokio::sync::{mpsc, watch};

use crate::{
    error::AppError,
    runner::service_relay::{InboundEvent, RelayCall},
};

use super::bounds::{MAX_SERVICE_HEADER_VALUE_BYTES, parse_header_entry};

/// Response headers that may cross the relay back to the caller.
const PERMITTED_RESPONSE_HEADERS: [(&str, &str); 3] = [
    ("content-type", "content-type"),
    ("cache-control", "cache-control"),
    ("retry-after", "retry-after"),
];

/// Cursor state for the response body stream.
struct ResponseCursor {
    call: RelayCall,
    events: mpsc::Receiver<InboundEvent>,
    terminal: watch::Receiver<Option<RelayErrorCode>>,
    done: bool,
}

/// Await the response head and stream the rest of the body.
///
/// Nothing is returned before a head exists: a stream that fails first is a
/// relay failure, reported as one, never as an empty success.
pub async fn relay_response(
    call: RelayCall,
    mut events: mpsc::Receiver<InboundEvent>,
) -> Result<Response<Body>, AppError> {
    let mut terminal = call.terminal_signal();
    let head = loop {
        tokio::select! {
            biased;
            changed = terminal.changed() => {
                if changed.is_err() {
                    return Err(refuse_before_head(None));
                }
                let code = terminal.borrow().unwrap_or(RelayErrorCode::UpstreamUnavailable);
                return Err(refuse_before_head(Some(code)));
            }
            event = events.recv() => {
                match event {
                    Some(InboundEvent::ResponseHead(head)) => break head,
                    Some(InboundEvent::Error(code)) => return Err(refuse_before_head(Some(code))),
                    Some(InboundEvent::Data(_)) | None => {
                        return Err(refuse_before_head(None));
                    }
                    Some(InboundEvent::End) => return Err(refuse_before_head(None)),
                }
            }
        }
    };

    let mut builder = Response::builder().status(head.status);
    for (name, value) in permitted_response_headers(&head) {
        builder = builder.header(name, value);
    }
    let cursor = ResponseCursor {
        call: call.clone(),
        events,
        terminal,
        done: false,
    };
    let body = Body::from_stream(futures_util::stream::unfold(
        cursor,
        |mut cursor| async move {
            if cursor.done {
                return None;
            }
            loop {
                tokio::select! {
                    biased;
                    changed = cursor.terminal.changed() => {
                        if changed.is_ok() {
                            cursor.done = true;
                            cursor.call.cancel();
                            let code = cursor.terminal.borrow().unwrap_or(RelayErrorCode::Cancelled);
                            return Some((Err(stream_failure(code)), cursor));
                        }
                        return None;
                    }
                    event = cursor.events.recv() => {
                        match event {
                            Some(InboundEvent::Data(bytes)) => return Some((Ok(bytes), cursor)),
                            // A second head cannot replace the status the caller
                            // already acted on.
                            Some(InboundEvent::ResponseHead(_)) => {
                                cursor.done = true;
                                cursor.call.cancel();
                                return Some((
                                    Err(stream_failure(RelayErrorCode::OutOfOrder)),
                                    cursor,
                                ));
                            }
                            Some(InboundEvent::End) => return None,
                            Some(InboundEvent::Error(code)) => {
                                cursor.done = true;
                                cursor.call.cancel();
                                return Some((Err(stream_failure(code)), cursor));
                            }
                            None => return None,
                        }
                    }
                }
            }
        },
    ));
    builder
        .body(body)
        .map_err(|error| AppError::internal(anyhow::Error::new(error)))
}

/// Copy back only the permitted response headers.
///
/// Entries arrive as `name: value`; a bare name carries no value and is
/// dropped rather than emitted empty, because fabricating a header is worse
/// than omitting one. Names are matched case-insensitively against a fixed
/// list, so an agent cannot smuggle `set-cookie`, `location`, or an
/// authorization header back to the caller by changing its spelling.
fn permitted_response_headers(head: &ResponseHeadFrame) -> Vec<(HeaderName, HeaderValue)> {
    let mut permitted = Vec::new();
    for declared in &head.allowed_headers {
        let Some((name, value)) = parse_header_entry(declared) else {
            continue;
        };
        if !PERMITTED_RESPONSE_HEADERS
            .iter()
            .any(|(allowed, _)| allowed.eq_ignore_ascii_case(&name))
        {
            continue;
        }
        if value.len() > MAX_SERVICE_HEADER_VALUE_BYTES
            || value.chars().any(|character| character.is_control())
        {
            continue;
        }
        let (Ok(header_name), Ok(header_value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(&value),
        ) else {
            continue;
        };
        permitted.push((header_name, header_value));
    }
    permitted
}

/// Map a relay refusal that arrived before any response head.
///
/// No agent diagnostic text is echoed: only the fixed code travels.
fn refuse_before_head(code: Option<RelayErrorCode>) -> AppError {
    match code {
        Some(RelayErrorCode::NotFound) => AppError::not_found("agent service resource not found"),
        Some(RelayErrorCode::Conflict) => AppError::conflict("agent service reported a conflict"),
        Some(RelayErrorCode::Timeout) => {
            AppError::service_unavailable("agent service request timed out")
        }
        Some(RelayErrorCode::Unauthorized) => {
            AppError::service_unavailable("agent service refused the relayed request")
        }
        _ => AppError::service_unavailable("the environment's manager relay is unavailable"),
    }
}

/// Body-stream failure. Carries the fixed relay code and nothing else.
fn stream_failure(code: RelayErrorCode) -> io::Error {
    io::Error::other(format!("relay stream ended: {}", code.as_relay_code()))
}

#[cfg(test)]
mod tests {
    use runner_protocol::service_relay::{RelayId, ResponseHeadFrame};
    use uuid::Uuid;

    use super::permitted_response_headers;

    fn head(status: u16, allowed_headers: Vec<String>) -> ResponseHeadFrame {
        ResponseHeadFrame {
            stream_id: RelayId::from_bytes(Uuid::new_v4().into_bytes()),
            status,
            allowed_headers,
        }
    }

    #[test]
    fn only_the_three_permitted_response_headers_survive() {
        let head = head(
            200,
            vec![
                "content-type: application/json".to_string(),
                "cache-control: no-store".to_string(),
                "Retry-After: 7".to_string(),
            ],
        );
        let permitted = permitted_response_headers(&head);
        let names: Vec<String> = permitted
            .iter()
            .map(|(name, _)| name.as_str().to_string())
            .collect();
        assert_eq!(names, ["content-type", "cache-control", "retry-after"]);
        assert_eq!(permitted[2].1, "7");
    }

    #[test]
    fn a_bare_name_carries_no_value_and_is_dropped() {
        let head = head(200, vec!["content-type".to_string()]);
        assert!(
            permitted_response_headers(&head).is_empty(),
            "emitting a header with no value would fabricate one"
        );
    }

    #[test]
    fn a_relay_cannot_navigate_or_re_authenticate_the_caller() {
        let head = head(
            302,
            vec![
                "set-cookie: session=secret".to_string(),
                "location: https://elsewhere.invalid".to_string(),
                "www-authenticate: Basic".to_string(),
                "transfer-encoding: chunked".to_string(),
            ],
        );
        assert!(
            permitted_response_headers(&head).is_empty(),
            "redirects, cookies and hop headers must never reach the caller"
        );
    }
}
