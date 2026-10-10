//! One live manager tunnel connection (issue 749 P2d).
//!
//! The connection task is the only place that touches the socket. It owns a
//! writer that drains the tunnel's outbound queue and a reader that hands
//! inbound frames to the registry. Neither loop interprets an agent's HTTP:
//! they move framed bytes between the relay and one authenticated manager.
//!
//! Admission happens before the upgrade, so a manager that already has a live
//! tunnel is refused with a conflict instead of replacing it. Cleanup always
//! names its own lease, so a connection tearing down late cannot detach the
//! tunnel that took over from it.
//!
//! Multiplex (`df-env-http.v2`): every frame in both directions carries its
//! own stream identifier, so inbound frames route by reading the frame and
//! verifying the authenticated manager *and* its lease against the stored
//! route. Unknown, cross-manager, or recently closed identifiers are
//! discarded without touching live streams; only global violations (oversize,
//! undecodable, or an inbound open) fail the tunnel closed.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use runner_protocol::service_relay::{
    ErrorFrame, MAX_WS_MESSAGE_BYTES, RelayControl, RelayErrorCode, RelayId, decode_chunk,
    decode_control,
};
use tokio::{
    sync::mpsc,
    time::{Duration, MissedTickBehavior, interval},
};
use uuid::Uuid;

use crate::{
    error::AppError,
    runner::service_relay::{
        RelayAdmissionError, RelayRegistry, StreamDelivery, TUNNEL_OUTBOUND_CAPACITY, TunnelHandle,
        frames::{InboundEvent, OutboundFrame, inbound_control},
    },
};

/// How often the control plane probes an otherwise silent tunnel.
pub const RELAY_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
/// Silence after which a tunnel is considered gone.
///
/// Twice the heartbeat: one missed probe is a hiccup, two is a dead
/// connection whose streams would otherwise wait forever.
pub const RELAY_INBOUND_SILENCE_LIMIT: Duration = Duration::from_secs(30);

/// An admitted tunnel, ready to be handed its socket.
pub struct TunnelAdmission {
    lease: RelayId,
    handle: TunnelHandle,
    outbound: mpsc::Receiver<OutboundFrame>,
}

/// Admit one tunnel for `manager_id`, or refuse with a conflict.
///
/// Runs before the upgrade so a refused connection never reaches the socket
/// and never displaces the live tunnel.
pub fn admit(registry: Arc<RelayRegistry>, manager_id: i64) -> Result<TunnelAdmission, AppError> {
    let (outbound_tx, outbound) = mpsc::channel(TUNNEL_OUTBOUND_CAPACITY);
    let handle = TunnelHandle::new(
        RelayId::from_bytes(Uuid::new_v4().into_bytes()),
        outbound_tx,
    );
    let lease = handle.lease();
    registry
        .attach_tunnel(manager_id, handle.clone())
        .map_err(|error| map_admission_error(error))?;
    Ok(TunnelAdmission {
        lease,
        handle,
        outbound,
    })
}

fn map_admission_error(error: RelayAdmissionError) -> AppError {
    match error {
        RelayAdmissionError::TunnelConflict => {
            AppError::conflict("a live service-relay tunnel already exists for this manager")
        }
        RelayAdmissionError::NoTunnel => {
            AppError::service_unavailable("the manager service-relay tunnel is unavailable")
        }
        RelayAdmissionError::StreamLimit | RelayAdmissionError::EpochChanged => {
            AppError::conflict("the manager service-relay tunnel cannot admit another stream")
        }
        RelayAdmissionError::DuplicateStream => {
            AppError::conflict("the relay stream identifier is already in use")
        }
        RelayAdmissionError::BodyTooLarge => {
            AppError::bad_request("declared body length exceeds the relay bound")
        }
    }
}

/// Drive one admitted tunnel until the connection ends.
pub async fn run(
    registry: Arc<RelayRegistry>,
    manager_id: i64,
    admission: TunnelAdmission,
    socket: WebSocket,
) {
    let TunnelAdmission {
        lease,
        handle,
        mut outbound,
    } = admission;
    let (sink, stream) = socket.split();
    let writer_handle = handle.clone();
    let writer = tokio::spawn(async move {
        write_frames(sink, &mut outbound, &writer_handle).await;
    });
    read_frames(stream, &registry, &handle, manager_id).await;
    // Stop the writer, then detach by lease. Detaching last means the
    // registry can never observe a tunnel whose socket is still writing.
    handle.abort();
    registry.detach_tunnel(manager_id, lease);
    let _ = writer.await;
}

/// Drain the outbound queue to the socket until the tunnel stops.
async fn write_frames(
    mut sink: futures_util::stream::SplitSink<WebSocket, Message>,
    outbound: &mut mpsc::Receiver<OutboundFrame>,
    handle: &TunnelHandle,
) {
    let mut heartbeat = interval(RELAY_HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        if handle.is_aborted() {
            break;
        }
        tokio::select! {
            _ = heartbeat.tick() => {
                if sink.send(Message::Ping(Default::default())).await.is_err() {
                    break;
                }
            }
            frame = outbound.recv() => {
                let Some(frame) = frame else { break };
                if handle.is_aborted() {
                    break;
                }
                let message = match frame {
                    OutboundFrame::Text(text) => Message::Text(text.into()),
                    OutboundFrame::Binary(bytes) => Message::Binary(bytes.into()),
                };
                if sink.send(message).await.is_err() {
                    break;
                }
            }
        }
    }
    let _ = sink.close().await;
}

/// Read inbound frames until the connection ends or falls silent.
async fn read_frames(
    mut stream: futures_util::stream::SplitStream<WebSocket>,
    registry: &Arc<RelayRegistry>,
    handle: &TunnelHandle,
    manager_id: i64,
) {
    loop {
        if handle.is_aborted() {
            break;
        }
        let silence = tokio::time::sleep(RELAY_INBOUND_SILENCE_LIMIT);
        tokio::pin!(silence);
        let message = tokio::select! {
            _ = &mut silence => {
                tracing::warn!("manager service-relay tunnel went silent");
                break;
            }
            message = stream.next() => message,
        };
        match message {
            None | Some(Err(_)) => break,
            Some(Ok(Message::Close(_))) => break,
            // Axum answers pings itself; a pong is connectivity, not payload.
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            Some(Ok(Message::Text(text))) => {
                if !dispatch_control(registry, handle, manager_id, text.as_str()).await {
                    break;
                }
            }
            Some(Ok(Message::Binary(bytes))) => {
                if !dispatch_data(registry, handle, manager_id, &bytes).await {
                    break;
                }
            }
        }
    }
}

/// Hand one control frame to the stream named by its own identifier.
///
/// A manager never opens a stream; only the control plane does, so an
/// inbound open fails the tunnel closed with an attributed refusal. Every
/// other frame routes by its required `stream_id` with the authenticated
/// manager and lease verified: unknown, cross-manager, stale-lease, or
/// recently closed identifiers are discarded so delayed frames can never
/// kill an unrelated live stream.
async fn dispatch_control(
    registry: &Arc<RelayRegistry>,
    handle: &TunnelHandle,
    manager_id: i64,
    raw: &str,
) -> bool {
    if raw.len() > MAX_WS_MESSAGE_BYTES {
        // No attributable identifier: fail closed without an Error frame,
        // since every v2 Error requires its stream.
        return false;
    }
    let control = match decode_control(raw) {
        Ok(control) => control,
        Err(_) => return false,
    };
    if let RelayControl::Open(open) = &control {
        return refuse_with(handle, open.stream_id, RelayErrorCode::UnknownFrame).await;
    }
    let stream_id = control.stream_id();
    let Some(event) = inbound_control(&control) else {
        // An acknowledgement with nothing to deliver is not a violation.
        return true;
    };
    match registry.deliver(manager_id, handle.lease(), stream_id, event) {
        StreamDelivery::Delivered | StreamDelivery::NoRoute => true,
        // The caller is not draining. Refusing the stream is how the relay
        // applies backpressure without ever buffering a whole response.
        StreamDelivery::Backpressure => {
            refuse_with(handle, stream_id, RelayErrorCode::QueueOverflow).await
        }
    }
}

/// Hand one data frame to its stream, applying backpressure by refusing the
/// stream rather than buffering it.
async fn dispatch_data(
    registry: &Arc<RelayRegistry>,
    handle: &TunnelHandle,
    manager_id: i64,
    raw: &[u8],
) -> bool {
    if raw.len() > MAX_WS_MESSAGE_BYTES {
        return false;
    }
    let chunk = match decode_chunk(raw) {
        Ok(chunk) => chunk,
        Err(_) => return false,
    };
    match registry.deliver(
        manager_id,
        handle.lease(),
        chunk.stream_id,
        InboundEvent::Data(axum::body::Bytes::copy_from_slice(chunk.data)),
    ) {
        // A frame for a stream that already finished is discarded: the
        // caller left before its peer stopped writing.
        StreamDelivery::NoRoute => true,
        StreamDelivery::Delivered => true,
        StreamDelivery::Backpressure => {
            refuse_with(handle, chunk.stream_id, RelayErrorCode::QueueOverflow).await
        }
    }
}

/// Report an attributed protocol violation on the tunnel and end it.
async fn refuse_with(handle: &TunnelHandle, stream_id: RelayId, code: RelayErrorCode) -> bool {
    if let Ok(frame) = OutboundFrame::control(&RelayControl::Error(ErrorFrame { stream_id, code }))
    {
        handle.try_send(frame);
    }
    false
}
