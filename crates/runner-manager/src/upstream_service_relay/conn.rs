//! One live relay tunnel on the manager side (issue 749 P2d).
//!
//! Owns the only socket this manager holds to the control plane: a writer
//! draining the bounded outbound queue and a reader routing inbound frames
//! to their streams by identifier. Neither loop interprets agent HTTP; they
//! move framed bytes between the tunnel and the streams that serve them. A
//! full outbound queue or a global protocol violation ends the connection
//! rather than dropping a frame the peer will wait for forever.
//!
//! Streams multiplex (`df-env-http.v2`): many assemble and many serve at
//! once, each answered on its own task, up to the fixed per-environment and
//! per-manager quotas. Per-stream failures carry their own identifier and
//! never end the tunnel; only global violations do.

use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use super::conn_state::ConnState;
use super::dispatch::{dispatch_control, dispatch_data};
use super::streams::{ExecStream, OutMsg};

/// Outbound frames buffered for one tunnel.
///
/// Bounded so a wedged socket cannot grow an unbounded queue. A full queue
/// ends the connection: dropping a control frame would strand the peer.
const OUTBOUND_CAPACITY: usize = 64;

/// Drive one connected tunnel until it ends.
pub(crate) async fn drive<E: ExecStream + 'static>(
    exec: Arc<E>,
    ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) {
    let (out_tx, mut out_rx) = mpsc::channel::<OutMsg>(OUTBOUND_CAPACITY);
    let (mut sink, mut stream) = ws.split();
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let message = match msg {
                OutMsg::Text(text) => Message::Text(text.into()),
                OutMsg::Binary(bytes) => Message::Binary(bytes.into()),
                OutMsg::Pong(payload) => Message::Pong(payload.into()),
            };
            if sink.send(message).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    let state = Arc::new(Mutex::new(ConnState::new()));
    read_frames(&exec, &mut stream, &out_tx, &state).await;
    // Everyone still in flight is abandoned with the tunnel: abort each
    // serve task so a late agent answer can never be written to a dead
    // socket, and drop every assembler so nothing lingers.
    {
        let mut guard = state.lock().expect("tunnel state poisoned");
        for (_, (_, task)) in guard.serving.drain() {
            task.abort();
        }
        guard.assembling.clear();
    }
    drop(out_tx);
    let _ = writer.await;
}

/// Route inbound frames until the connection ends or violates the protocol.
async fn read_frames<E: ExecStream + 'static, S>(
    exec: &Arc<E>,
    stream: &mut S,
    out: &mpsc::Sender<OutMsg>,
    state: &Arc<Mutex<ConnState>>,
) where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let message = match stream.next().await {
            Some(Ok(message)) => message,
            Some(Err(_)) | None => break,
        };
        let keep = match message {
            Message::Text(text) => dispatch_control(exec, state, &text, out).await,
            Message::Binary(bytes) => dispatch_data(state, &bytes, out).await,
            // The control plane probes an idle tunnel; answering keeps the
            // tunnel observably alive in both directions.
            Message::Ping(payload) => out.send(OutMsg::Pong(payload.to_vec())).await.is_ok(),
            Message::Pong(_) => true,
            Message::Close(_) | Message::Frame(_) => break,
        };
        if !keep {
            break;
        }
    }
}
