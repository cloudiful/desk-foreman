//! Tunnel frame routing on the manager side (issue 749 P2d).
//!
//! Split from [`super::conn`]: this owns the per-tunnel stream tables and
//! every inbound-frame decision, while the connection owns the socket and
//! the writer. Routing never awaits agent HTTP; serving runs on per-stream
//! tasks spawned from here.
//!
//! Multiplex (`df-env-http.v2`): request bodies assemble in parallel by
//! identifier — `Open`, data frames and `RequestEnd` each carry their own
//! stream — while any number serve concurrently under the fixed quotas.
//! Every refusal carries the offending stream's identifier so one stream's
//! failure can never be mistaken for another's. Recently closed identifiers
//! are remembered in a bounded set to refuse reuse and to discard delayed
//! frames without touching live streams.

use std::sync::{Arc, Mutex};

use runner_protocol::service_relay::{
    MAX_WS_MESSAGE_BYTES, OpenFrame, RelayControl, RelayErrorCode, RelayId, StreamFrame,
    admit_stream, decode_control, encode_control,
};
use tokio::sync::mpsc;

use super::conn_state::ConnState;
use super::streams::{ExecStream, OutMsg, StreamSlot, decode_data_frame, serve_slot};

/// Handle one inbound control frame.
pub(crate) async fn dispatch_control<E: ExecStream + 'static>(
    exec: &Arc<E>,
    state: &Arc<Mutex<ConnState>>,
    raw: &str,
    out: &mpsc::Sender<OutMsg>,
) -> bool {
    if raw.len() > MAX_WS_MESSAGE_BYTES {
        return false;
    }
    let control = match decode_control(raw) {
        Ok(control) => control,
        Err(_) => return false,
    };
    match control {
        RelayControl::Open(open) => dispatch_open(state, open, out).await,
        RelayControl::RequestEnd(frame) => dispatch_request_end(exec, state, frame, out),
        RelayControl::Cancel(frame) => dispatch_cancel(state, frame),
        // The control plane never answers; anything response-shaped from
        // that direction is a violation.
        RelayControl::ResponseHead(_) | RelayControl::End(_) | RelayControl::Error(_) => false,
    }
}

/// Start assembling one stream.
///
/// A reused identifier — live or recently closed — is refused with an
/// attributed fixed error while the tunnel stays up; a quota overflow or an
/// oversized declaration is refused the same way. A malformed open tears
/// the tunnel down, since no identifier can be attributed safely.
pub(crate) async fn dispatch_open(
    state: &Arc<Mutex<ConnState>>,
    open: OpenFrame,
    out: &mpsc::Sender<OutMsg>,
) -> bool {
    enum Decision {
        Refuse(RelayErrorCode),
        Assemble,
        Violation,
    }
    let stream_id = open.stream_id;
    let decision = {
        let guard = state.lock().expect("tunnel state poisoned");
        if guard.assembling.contains_key(&stream_id)
            || guard.serving.contains_key(&stream_id)
            || guard.is_closed(stream_id)
        {
            Decision::Refuse(RelayErrorCode::Conflict)
        } else {
            match check_quotas(&guard, &open) {
                Ok(()) => Decision::Assemble,
                Err(code) => Decision::Refuse(code),
            }
        }
    };
    match decision {
        Decision::Refuse(code) => send_fixed_error(out, stream_id, code).await,
        Decision::Violation => false,
        Decision::Assemble => {
            let slot = match StreamSlot::new(open) {
                Ok(slot) => slot,
                Err(_) => return false,
            };
            let id = slot.stream_id();
            state
                .lock()
                .expect("tunnel state poisoned")
                .assembling
                .insert(id, slot);
            true
        }
    }
}

/// Count one more stream against the fixed quotas.
///
/// Assembling bodies and serving responses both count: a slow body and a
/// long-lived event stream occupy the same bounded slots.
fn check_quotas(guard: &ConnState, open: &OpenFrame) -> Result<(), RelayErrorCode> {
    let mut environment_streams = 0usize;
    for slot in guard.assembling.values() {
        if slot.environment_id() == open.environment_id {
            environment_streams += 1;
        }
    }
    for (env, _) in guard.serving.values() {
        if *env == open.environment_id {
            environment_streams += 1;
        }
    }
    let manager_streams = guard.assembling.len() + guard.serving.len();
    admit_stream(environment_streams, manager_streams).map_err(|_| RelayErrorCode::StreamLimit)?;
    if open.body_length > runner_protocol::service_relay::MAX_BODY_BYTES {
        return Err(RelayErrorCode::BodyTooLarge);
    }
    Ok(())
}

/// Close one assembling request and serve it on its own task, if exactly
/// its declared body arrived.
///
/// Unknown, already-serving, or recently closed identifiers are discarded:
/// a delayed duplicate completion must never kill an unrelated live stream.
/// A short or oversized body fails only its own stream with an attributed
/// error while the tunnel stays up. Serving never blocks the reader.
pub(crate) fn dispatch_request_end<E: ExecStream + 'static>(
    exec: &Arc<E>,
    state: &Arc<Mutex<ConnState>>,
    frame: StreamFrame,
    out: &mpsc::Sender<OutMsg>,
) -> bool {
    let id = frame.stream_id;
    let assembled = {
        let mut guard = state.lock().expect("tunnel state poisoned");
        let Some(slot) = guard.assembling.get_mut(&id) else {
            // Delayed, duplicate, or forged completion: isolated, not fatal.
            return true;
        };
        match slot.finish_request(&frame) {
            Ok(assembled) => Some(assembled),
            Err(code) => {
                let code = match code {
                    runner_protocol::service_relay::RelayProtocolError::BodyLengthMismatch
                    | runner_protocol::service_relay::RelayProtocolError::BodyTooLarge => {
                        RelayErrorCode::BodyLengthMismatch
                    }
                    _ => RelayErrorCode::OutOfOrder,
                };
                guard.assembling.remove(&id);
                guard.remember_closed(id);
                drop(guard);
                // Owned send outside the lock: the queue is bounded and the
                // failure path must not hold stream tables across an await.
                // A full queue here still keeps the tunnel: the failure is
                // per-stream, and the next frame will observe the close.
                let out = out.clone();
                // Best-effort: the stream is already forgotten, so a wedged
                // queue must not wedge the reader behind it.
                let _ = out.try_send(attributed_error(id, code));
                return true;
            }
        }
    };
    let Some((open, body)) = assembled else {
        return true;
    };
    let env = open.environment_id;
    let state_clone = Arc::clone(state);
    let out_clone = out.clone();
    let exec_clone = Arc::clone(exec);
    let task = tokio::spawn(async move {
        serve_slot(exec_clone, open, body, out_clone).await;
        // Release this stream's quota slot exactly once, however it ends,
        // and remember it so its identifier cannot be immediately reused.
        let mut guard = state_clone.lock().expect("tunnel state poisoned");
        guard.serving.remove(&id);
        guard.remember_closed(id);
    });
    {
        let mut guard = state.lock().expect("tunnel state poisoned");
        guard.assembling.remove(&id);
        guard.serving.insert(id, (env, task.abort_handle()));
    }
    true
}

/// Cancel exactly the named stream.
///
/// Assembling bodies are forgotten, serving tasks are aborted and forgotten;
/// unknown or already-closed identifiers are acknowledged without effect so
/// delayed cancels can never kill an unrelated live stream.
pub(crate) fn dispatch_cancel(state: &Arc<Mutex<ConnState>>, frame: StreamFrame) -> bool {
    let mut guard = state.lock().expect("tunnel state poisoned");
    let id = frame.stream_id;
    if guard.assembling.remove(&id).is_some() {
        guard.remember_closed(id);
        return true;
    }
    if let Some((_, task)) = guard.serving.remove(&id) {
        task.abort();
        guard.remember_closed(id);
        return true;
    }
    // Nothing to cancel is an ack, whether unknown or already closed.
    true
}

/// Handle one inbound data frame, routed by its stream identifier.
///
/// A frame for a stream that already finished, is serving, or never
/// existed is discarded: the caller left before its peer stopped writing,
/// or the identifier is forged. An oversized or out-of-order slice fails
/// only its own assembling stream with an attributed error.
pub(crate) async fn dispatch_data(
    state: &Arc<Mutex<ConnState>>,
    raw: &[u8],
    out: &mpsc::Sender<OutMsg>,
) -> bool {
    if raw.len() > MAX_WS_MESSAGE_BYTES {
        return false;
    }
    let (id, payload) = match decode_data_frame(raw) {
        Ok(frame) => frame,
        Err(_) => return false,
    };
    let mut guard = state.lock().expect("tunnel state poisoned");
    let Some(slot) = guard.assembling.get_mut(&id) else {
        // Serving, closed, or unknown: isolated, not fatal.
        return true;
    };
    match slot.push_data(&payload) {
        Ok(()) => true,
        Err(_) => {
            guard.assembling.remove(&id);
            guard.remember_closed(id);
            drop(guard);
            // Per-stream failure, tunnel stays up. Best-effort: the slot is
            // already forgotten, so a wedged queue must not wedge the
            // reader behind it.
            let _ = out.try_send(attributed_error(id, RelayErrorCode::BodyLengthMismatch));
            true
        }
    }
}

/// Send one attributed fixed error frame without touching any stream's quota.
async fn send_fixed_error(
    out: &mpsc::Sender<OutMsg>,
    stream_id: RelayId,
    code: RelayErrorCode,
) -> bool {
    use runner_protocol::service_relay::ErrorFrame;
    let text = match encode_control(&RelayControl::Error(ErrorFrame { stream_id, code })) {
        Ok(text) => text,
        Err(_) => return false,
    };
    out.send(OutMsg::Text(text)).await.is_ok()
}

/// Attributed error frame for lock-free failure paths.
fn attributed_error(stream_id: RelayId, code: RelayErrorCode) -> OutMsg {
    use runner_protocol::service_relay::ErrorFrame;
    match encode_control(&RelayControl::Error(ErrorFrame { stream_id, code })) {
        Ok(text) => OutMsg::Text(text),
        Err(_) => OutMsg::Text(
            encode_control(&RelayControl::Error(ErrorFrame {
                stream_id,
                code: RelayErrorCode::UpstreamError,
            }))
            .expect("a fixed error frame always encodes"),
        ),
    }
}
