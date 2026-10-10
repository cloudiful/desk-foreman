//! One relayed stream on the manager side (issue 749 P2d).
//!
//! Assembles the request DF forwards — `Open`, bounded data frames, then
//! `RequestEnd` — and answers with `ResponseHead`, data frames, and `End`
//! (or one fixed-code `Error`), every control frame carrying its own stream
//! identifier. The declared body length is authoritative: nothing reaches
//! the private agent until exactly those bytes arrived, and a short or
//! oversized body is refused instead of served. No response is ever
//! synthesized after the tunnel is gone: a dead outbound queue ends the
//! connection rather than inventing bytes.

use runner_protocol::service_relay::{
    BodyLengthGuard, OpenFrame, RelayControl, RelayErrorCode, RelayId, RelayProtocolError,
    StreamFrame, StreamOrder, decode_chunk,
};

use super::respond::{answer, send_error};
use crate::runtime::DockerRunnerBackend;
use crate::runtime::execution_environment::ExecutionEnvironmentProfile;
use crate::runtime::execution_environment::service::relay;

/// One frame queued for the tunnel writer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum OutMsg {
    Text(String),
    Binary(Vec<u8>),
    Pong(Vec<u8>),
}

/// Assembly state for one stream DF opened.
pub(crate) struct StreamSlot {
    order: StreamOrder,
    guard: BodyLengthGuard,
    open: OpenFrame,
    body: Vec<u8>,
}

impl StreamSlot {
    /// Start assembling from an `Open` frame.
    pub(crate) fn new(open: OpenFrame) -> Result<Self, RelayProtocolError> {
        let mut order = StreamOrder::new();
        order.accept(&RelayControl::Open(open.clone()))?;
        let guard = BodyLengthGuard::new(open.body_length)?;
        Ok(Self {
            order,
            guard,
            open,
            body: Vec::new(),
        })
    }

    pub(crate) fn stream_id(&self) -> RelayId {
        self.open.stream_id
    }

    pub(crate) fn environment_id(&self) -> RelayId {
        self.open.environment_id
    }

    /// Append one data frame's payload, refusing anything over the declared
    /// length or after the request completed.
    pub(crate) fn push_data(&mut self, payload: &[u8]) -> Result<(), RelayProtocolError> {
        if self.order.phase() != runner_protocol::service_relay::StreamPhase::Request {
            return Err(RelayProtocolError::OutOfOrder);
        }
        self.guard.push(payload.len())?;
        self.body.extend_from_slice(payload);
        Ok(())
    }

    /// Close the request, returning the assembled `(open, body)` only when
    /// exactly the declared bytes arrived.
    pub(crate) fn finish_request(
        &mut self,
        frame: &StreamFrame,
    ) -> Result<(OpenFrame, Vec<u8>), RelayProtocolError> {
        if frame.stream_id != self.open.stream_id {
            return Err(RelayProtocolError::InvalidIdentifier);
        }
        self.order
            .accept(&RelayControl::RequestEnd(frame.clone()))?;
        self.guard.finish()?;
        Ok((self.open.clone(), std::mem::take(&mut self.body)))
    }
}

/// Decode one inbound binary frame for its stream, refusing anything the
/// peer must never send.
pub(crate) fn decode_data_frame(raw: &[u8]) -> Result<(RelayId, Vec<u8>), RelayProtocolError> {
    let chunk = decode_chunk(raw)?;
    Ok((chunk.stream_id, chunk.data.to_vec()))
}

/// How one assembled stream reaches its agent.
///
/// Production builds the context from real backend admission; tests build a
/// scoped context aimed at an ephemeral loopback agent with a real stored
/// credential and a real sticky cancel. The tunnel dispatcher only sees
/// this trait, so no test surface exists in the production path.
pub(crate) trait ExecStream: Send + Sync {
    fn exec(
        &self,
        open: OpenFrame,
        body: Vec<u8>,
        out: tokio::sync::mpsc::Sender<OutMsg>,
    ) -> impl std::future::Future<Output = ()> + Send;
}

/// Production execution: admit against the durable barrier and the live
/// container, then call the derived private target.
pub(crate) struct ProdExec {
    docker: std::sync::Arc<DockerRunnerBackend>,
    profile: std::sync::Arc<ExecutionEnvironmentProfile>,
}

impl ProdExec {
    pub(crate) fn new(
        docker: std::sync::Arc<DockerRunnerBackend>,
        profile: std::sync::Arc<ExecutionEnvironmentProfile>,
    ) -> Self {
        Self { docker, profile }
    }
}

impl ExecStream for ProdExec {
    async fn exec(&self, open: OpenFrame, body: Vec<u8>, out: tokio::sync::mpsc::Sender<OutMsg>) {
        let environment_id = open.environment_id.to_canonical();
        let admitted = relay::admit_service_stream(
            &self.docker,
            &self.profile,
            &environment_id,
            open.generation,
        )
        .await;
        let admitted = match admitted {
            Ok(admitted) => admitted,
            Err(_) => {
                send_error(&out, open.stream_id, RelayErrorCode::UpstreamUnavailable).await;
                return;
            }
        };
        answer(&open, body, &admitted, &out).await;
    }
}

/// Serve one assembled stream: admit, call the private agent, and answer.
///
/// Sends exactly one terminal frame (`End` or `Error`). Any send failure
/// means the tunnel is gone, so the caller tears the connection down
/// instead of retrying on a dead socket.
pub(crate) async fn serve_slot<E: ExecStream + 'static>(
    exec: std::sync::Arc<E>,
    open: OpenFrame,
    body: Vec<u8>,
    out: tokio::sync::mpsc::Sender<OutMsg>,
) {
    exec.exec(open, body, out).await;
}

#[cfg(test)]
mod tests {
    use runner_protocol::service_relay::{
        MAX_BODY_BYTES, OpenFrame, RelayId, RelayProtocolError, StreamFrame,
    };
    use uuid::Uuid;

    use super::StreamSlot;

    fn test_open(body_length: u64) -> OpenFrame {
        OpenFrame {
            stream_id: RelayId::from_bytes(Uuid::new_v4().into_bytes()),
            environment_id: RelayId::from_bytes(Uuid::new_v4().into_bytes()),
            generation: 4,
            method: "POST".to_string(),
            path: "v1/chat".to_string(),
            query: None,
            allowed_headers: Vec::new(),
            body_length,
        }
    }

    fn end_for(open: &OpenFrame) -> StreamFrame {
        StreamFrame {
            stream_id: open.stream_id,
        }
    }

    #[test]
    fn a_well_formed_request_assembles_exactly() {
        let open = test_open(3);
        let end = end_for(&open);
        let mut slot = StreamSlot::new(open).expect("open");
        slot.push_data(b"ab").expect("data");
        slot.push_data(b"c").expect("data");
        let (_, body) = slot.finish_request(&end).expect("complete");
        assert_eq!(body, b"abc");
    }

    #[test]
    fn an_oversized_body_is_refused_not_buffered() {
        let mut open = test_open(1);
        open.body_length = 1;
        let mut slot = StreamSlot::new(open).expect("open");
        assert_eq!(
            slot.push_data(b"ab").err(),
            Some(RelayProtocolError::BodyLengthMismatch)
        );
    }

    #[test]
    fn a_short_body_never_completes() {
        let open = test_open(4);
        let end = end_for(&open);
        let mut slot = StreamSlot::new(open).expect("open");
        slot.push_data(b"ab").expect("data");
        assert_eq!(
            slot.finish_request(&end).err(),
            Some(RelayProtocolError::BodyLengthMismatch)
        );
    }

    #[test]
    fn data_after_completion_is_out_of_order() {
        let open = test_open(0);
        let end = end_for(&open);
        let mut slot = StreamSlot::new(open).expect("open");
        slot.finish_request(&end).expect("empty body completes");
        assert_eq!(
            slot.push_data(b"x").err(),
            Some(RelayProtocolError::OutOfOrder)
        );
        assert!(slot.finish_request(&end).is_err(), "completion is one-shot");
    }

    #[test]
    fn a_completion_for_another_stream_is_refused() {
        let open = test_open(0);
        let mut slot = StreamSlot::new(open).expect("open");
        let foreign = StreamFrame {
            stream_id: RelayId::from_bytes(Uuid::new_v4().into_bytes()),
        };
        assert_eq!(
            slot.finish_request(&foreign).err(),
            Some(RelayProtocolError::InvalidIdentifier)
        );
    }

    #[test]
    fn an_oversized_declaration_is_refused_at_open() {
        let mut open = test_open(0);
        open.body_length = MAX_BODY_BYTES + 1;
        assert!(StreamSlot::new(open).is_err());
    }
}
