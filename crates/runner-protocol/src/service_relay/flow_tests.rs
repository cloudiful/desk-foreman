//! Relay ordering, body-length and concurrency tests (issue 749 P2a,
//! `#[cfg(test)]`).
//!
//! Covers the state machines a receiver must run: frame order, one-shot
//! frames, body-length agreement, admission caps, and bounded queue depth.

#[cfg(test)]
mod flow_tests {
    use crate::service_relay::control::{ResponseHeadFrame, StreamFrame};
    use crate::service_relay::*;

    fn stream() -> RelayId {
        RelayId::from_bytes([0x11; 16])
    }

    fn frame() -> StreamFrame {
        StreamFrame {
            stream_id: stream(),
        }
    }

    fn head(status: u16) -> RelayControl {
        RelayControl::ResponseHead(ResponseHeadFrame {
            stream_id: stream(),
            status,
            allowed_headers: vec![],
        })
    }

    fn open(body_length: u64) -> RelayControl {
        RelayControl::Open(OpenFrame {
            stream_id: RelayId::from_bytes([0x11; 16]),
            environment_id: RelayId::from_bytes([0x22; 16]),
            generation: 1,
            method: "POST".to_string(),
            path: "/v1/chat".to_string(),
            query: None,
            allowed_headers: vec![],
            body_length,
        })
    }

    #[test]
    fn stream_order_accepts_the_happy_path_once() {
        let mut order = StreamOrder::new();
        assert_eq!(order.accept(&open(0)), Ok(()));
        assert_eq!(order.phase(), StreamPhase::Request);
        assert_eq!(order.accept(&RelayControl::RequestEnd(frame())), Ok(()));
        assert_eq!(order.phase(), StreamPhase::RequestComplete);
        assert_eq!(order.accept(&head(200)), Ok(()));
        assert_eq!(order.phase(), StreamPhase::ResponseBody);
        assert_eq!(order.accept(&RelayControl::End(frame())), Ok(()));
        assert_eq!(order.phase(), StreamPhase::Closed);
    }

    #[test]
    fn stream_order_refuses_repeats_and_terminal_traffic() {
        let mut order = StreamOrder::new();
        assert_eq!(order.accept(&open(0)), Ok(()));
        assert_eq!(
            order.accept(&open(0)),
            Err(RelayProtocolError::OutOfOrder),
            "a second open on one stream is refused"
        );
        assert_eq!(order.accept(&RelayControl::RequestEnd(frame())), Ok(()));
        assert_eq!(
            order.accept(&RelayControl::RequestEnd(frame())),
            Err(RelayProtocolError::OutOfOrder),
            "request end is one-shot"
        );
        assert_eq!(order.accept(&RelayControl::End(frame())), Ok(()));
        for late in [
            RelayControl::Cancel(frame()),
            RelayControl::End(frame()),
            RelayControl::RequestEnd(frame()),
            head(200),
        ] {
            assert_eq!(
                order.accept(&late),
                Err(RelayProtocolError::OutOfOrder),
                "nothing may follow a terminal frame"
            );
        }
    }

    #[test]
    fn response_head_cannot_precede_request_end() {
        let mut order = StreamOrder::new();
        assert_eq!(order.accept(&open(0)), Ok(()));
        assert_eq!(
            order.accept(&head(200)),
            Err(RelayProtocolError::OutOfOrder)
        );
    }

    #[test]
    fn body_length_must_match_what_arrived() {
        let mut guard = BodyLengthGuard::new(10).expect("declared length within cap");
        assert_eq!(guard.push(4), Ok(()));
        assert_eq!(guard.received(), 4);
        assert_eq!(
            guard.finish(),
            Err(RelayProtocolError::BodyLengthMismatch),
            "a short body cannot be finished"
        );
        assert_eq!(guard.push(6), Ok(()));
        assert_eq!(guard.finish(), Ok(()));
        assert_eq!(
            guard.push(1),
            Err(RelayProtocolError::BodyLengthMismatch),
            "bytes past the declared length are refused"
        );
    }

    #[test]
    fn body_length_above_the_global_cap_is_refused_at_declaration() {
        assert!(
            BodyLengthGuard::new(MAX_BODY_BYTES).is_ok(),
            "the cap is allowed"
        );
        assert_eq!(
            BodyLengthGuard::new(MAX_BODY_BYTES + 1).unwrap_err(),
            RelayProtocolError::BodyTooLarge
        );
    }

    #[test]
    fn concurrency_caps_are_inclusive_boundaries() {
        assert_eq!(admit_stream(0, 0), Ok(()));
        assert_eq!(
            admit_stream(
                MAX_ACTIVE_STREAMS_PER_ENVIRONMENT - 1,
                MAX_ACTIVE_STREAMS_PER_MANAGER - 1
            ),
            Ok(())
        );
        assert_eq!(
            admit_stream(MAX_ACTIVE_STREAMS_PER_ENVIRONMENT, 0),
            Err(RelayProtocolError::StreamLimit),
            "the per-environment cap is enforced"
        );
        assert_eq!(
            admit_stream(0, MAX_ACTIVE_STREAMS_PER_MANAGER),
            Err(RelayProtocolError::StreamLimit),
            "the per-manager cap is enforced"
        );
    }

    #[test]
    fn chunk_queue_applies_backpressure_instead_of_growing() {
        let mut queue = ChunkQueueDepth::new();
        for _ in 0..MAX_QUEUED_CHUNKS_PER_STREAM {
            assert_eq!(queue.push(), Ok(()));
        }
        assert_eq!(queue.depth(), MAX_QUEUED_CHUNKS_PER_STREAM);
        assert_eq!(queue.push(), Err(RelayProtocolError::QueueOverflow));
        queue.pop();
        assert_eq!(queue.depth(), MAX_QUEUED_CHUNKS_PER_STREAM - 1);
        assert_eq!(
            queue.push(),
            Ok(()),
            "releasing one chunk frees exactly one slot"
        );
        for _ in 0..MAX_QUEUED_CHUNKS_PER_STREAM {
            queue.pop();
        }
        assert_eq!(queue.depth(), 0);
        queue.pop();
        assert_eq!(queue.depth(), 0, "popping an empty queue is a no-op");
    }

    #[test]
    fn protocol_errors_carry_stable_codes() {
        for (error, code) in [
            (RelayProtocolError::InvalidIdentifier, "invalid_identifier"),
            (RelayProtocolError::MalformedFrame, "malformed_frame"),
            (RelayProtocolError::OutOfOrder, "out_of_order"),
            (RelayProtocolError::BodyTooLarge, "body_too_large"),
            (
                RelayProtocolError::BodyLengthMismatch,
                "body_length_mismatch",
            ),
            (RelayProtocolError::StreamLimit, "stream_limit"),
            (RelayProtocolError::QueueOverflow, "queue_overflow"),
        ] {
            assert_eq!(error.code(), code);
            assert_eq!(error.to_string(), code);
            assert_eq!(RelayErrorCode::from(error).as_relay_code(), code);
        }
    }
}
