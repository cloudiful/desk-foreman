//! Wire answers for one served stream (issue 749 P2d).
//!
//! Split from [`super::streams`]: shared by production and test execution
//! so the framing under test is the framing that ships. Sends exactly one
//! terminal frame per stream; any send failure means the tunnel is gone.
//!
//! Every control frame carries its own stream identifier (`df-env-http.v2`),
//! so multiplexed answers correlate by reading the frame rather than by
//! tunnel-singleton position.

use runner_protocol::service_relay::{
    MAX_CHUNK_DATA_BYTES, OpenFrame, RelayControl, RelayErrorCode, RelayId, ResponseHeadFrame,
    StreamFrame, encode_chunk, encode_control,
};

use super::streams::OutMsg;
use crate::runtime::execution_environment::service::{http, relay};

/// Answer one stream from an already-admitted context.
///
/// Shared by production and test execution so the wire framing under test
/// is the framing that ships. Sends exactly one terminal frame, always
/// attributed to `open.stream_id`.
pub(crate) async fn answer(
    open: &OpenFrame,
    body: Vec<u8>,
    admitted: &relay::AdmittedService,
    out: &tokio::sync::mpsc::Sender<OutMsg>,
) {
    let result = answer_inner(open, body, admitted, out).await;
    if let Err(code) = result {
        send_error(out, open.stream_id, code).await;
    }
}

/// Send one attributed refusal for `stream_id`.
pub(crate) async fn send_error(
    out: &tokio::sync::mpsc::Sender<OutMsg>,
    stream_id: RelayId,
    code: RelayErrorCode,
) {
    let _ = out
        .send(control_msg(
            &RelayControl::Error(runner_protocol::service_relay::ErrorFrame { stream_id, code }),
            stream_id,
        ))
        .await;
}

async fn answer_inner(
    open: &OpenFrame,
    body: Vec<u8>,
    admitted: &relay::AdmittedService,
    out: &tokio::sync::mpsc::Sender<OutMsg>,
) -> Result<(), RelayErrorCode> {
    let reply = http::send_to_agent(
        &admitted,
        &open.method,
        &open.path,
        open.query.as_deref(),
        &open.allowed_headers,
        body,
        &admitted.cancel,
    )
    .await
    .map_err(|error| match error {
        http::AgentError::Cancelled => RelayErrorCode::Cancelled,
        http::AgentError::NotFound => RelayErrorCode::NotFound,
        http::AgentError::Timeout => RelayErrorCode::Timeout,
        http::AgentError::Unavailable => RelayErrorCode::UpstreamUnavailable,
    })?;
    out.send(control_msg(
        &RelayControl::ResponseHead(ResponseHeadFrame {
            stream_id: open.stream_id,
            status: reply.status,
            allowed_headers: reply.headers,
        }),
        open.stream_id,
    ))
    .await
    .map_err(|_| RelayErrorCode::UpstreamUnavailable)?;
    let mut body = reply.body;
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|_| RelayErrorCode::UpstreamError)?;
        for slice in chunk.chunks(MAX_CHUNK_DATA_BYTES) {
            let frame =
                encode_chunk(open.stream_id, slice).map_err(|_| RelayErrorCode::UpstreamError)?;
            out.send(OutMsg::Binary(frame))
                .await
                .map_err(|_| RelayErrorCode::UpstreamUnavailable)?;
        }
    }
    out.send(control_msg(
        &RelayControl::End(StreamFrame {
            stream_id: open.stream_id,
        }),
        open.stream_id,
    ))
    .await
    .map_err(|_| RelayErrorCode::UpstreamUnavailable)?;
    Ok(())
}

fn control_msg(control: &RelayControl, fallback_id: RelayId) -> OutMsg {
    match encode_control(control) {
        Ok(text) => OutMsg::Text(text),
        Err(_) => OutMsg::Text(
            encode_control(&RelayControl::Error(
                runner_protocol::service_relay::ErrorFrame {
                    stream_id: fallback_id,
                    code: RelayErrorCode::UpstreamError,
                },
            ))
            .expect("a fixed error frame always encodes"),
        ),
    }
}
