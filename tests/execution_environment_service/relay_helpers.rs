//! Shared mock-manager helpers for the relay suites (issue 749 P2d).
//!
//! Split from `relay_cases.rs` so no test file exceeds the size budget. The
//! mock manager holds a real relay tunnel and answers with real relay frames;
//! these helpers prove the control plane's behaviour and nothing about Docker
//! or a real agent. Every control frame carries its own stream identifier
//! (`df-env-http.v2`).

use std::time::Duration;

use runner_protocol::service_relay::{
    RelayControl, RelayId, ResponseHeadFrame, StreamFrame, decode_control, encode_chunk,
    encode_control,
};

use super::support::{HttpOutcome, ServiceFixture, call};
use super::ws_client::{Frame, WsClient};

pub const PROTOCOL: &str = "df-env-http.v2";

pub fn fence_header(generation: i64) -> (String, String) {
    (
        "X-Execution-Environment-Generation".to_string(),
        generation.to_string(),
    )
}

/// One relayed request, driven from the mock manager's side.
pub async fn relay(
    fixture: &ServiceFixture,
    method: &str,
    suffix: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> (tokio::task::JoinHandle<HttpOutcome>, WsClient) {
    let tunnel = WsClient::connect(&fixture.base, &fixture.manager_token, PROTOCOL)
        .await
        .expect("mock manager tunnel");
    let base = fixture.base.clone();
    let path = fixture.service_path(suffix);
    let token = fixture.application_token.clone();
    let method = method.to_string();
    let mut all_headers: Vec<(String, String)> = vec![fence_header(4)];
    all_headers.extend_from_slice(headers);
    let call_body = body.to_vec();
    let request = tokio::spawn(async move {
        call(
            &base,
            &method,
            &path,
            Some(&token),
            &all_headers,
            &call_body,
        )
        .await
    });
    (request, tunnel)
}

/// Read frames until the stream is opened.
///
/// Frames for a stream that already finished may still be in flight — a
/// completion marker from the previous request, for instance — so anything
/// that is not an opening frame is stepped over rather than mistaken for one.
pub async fn read_open(tunnel: &mut WsClient) -> (RelayId, OpenObserved) {
    loop {
        let frame = tunnel
            .next_frame(Duration::from_secs(10))
            .await
            .expect("an opening frame");
        let raw = match frame {
            Frame::Text(text) => text,
            other => panic!("expected a control frame, got {other:?}"),
        };
        match decode_control(&raw).expect("decodable control frame") {
            RelayControl::Open(open) => {
                let observed = OpenObserved {
                    method: open.method.clone(),
                    path: open.path.clone(),
                    query: open.query.clone(),
                    allowed_headers: open.allowed_headers.clone(),
                    body_length: open.body_length,
                };
                return (open.stream_id, observed);
            }
            _ => continue,
        }
    }
}

pub struct OpenObserved {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub allowed_headers: Vec<String>,
    pub body_length: u64,
}

pub async fn respond(
    tunnel: &mut WsClient,
    stream: runner_protocol::service_relay::RelayId,
    status: u16,
    headers: Vec<String>,
    body: &[u8],
) {
    tunnel
        .send_text(
            &encode_control(&RelayControl::ResponseHead(ResponseHeadFrame {
                stream_id: stream,
                status,
                allowed_headers: headers,
            }))
            .expect("encode response head"),
        )
        .await;
    if !body.is_empty() {
        tunnel
            .send_binary(&encode_chunk(stream, body).expect("encode chunk"))
            .await;
    }
    tunnel
        .send_text(
            &encode_control(&RelayControl::End(StreamFrame { stream_id: stream }))
                .expect("encode end"),
        )
        .await;
}

/// Send one attributed refusal for `stream`.
pub async fn refuse(
    tunnel: &mut WsClient,
    stream: RelayId,
    code: runner_protocol::service_relay::RelayErrorCode,
) {
    use runner_protocol::service_relay::ErrorFrame;
    tunnel
        .send_text(
            &encode_control(&RelayControl::Error(ErrorFrame {
                stream_id: stream,
                code,
            }))
            .expect("encode refusal"),
        )
        .await;
}
