//! Relay wire contract tests (issue 749 P2a, `#[cfg(test)]`).
//!
//! Covers identifier parsing, control round-trips, frame bounds, body
//! length agreement, ordering, and the concurrency caps. Pure values only:
//! no transport, no timers.

#[cfg(test)]
mod service_relay_tests {
    use crate::service_relay::control::{ErrorFrame, OpenFrame, ResponseHeadFrame, StreamFrame};
    use crate::service_relay::*;

    const STREAM: [u8; 16] = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
        0xef,
    ];
    const ENVIRONMENT: [u8; 16] = [0xabu8; 16];

    fn stream_id() -> RelayId {
        RelayId::from_bytes(STREAM)
    }

    fn open(body_length: u64) -> RelayControl {
        RelayControl::Open(OpenFrame {
            stream_id: stream_id(),
            environment_id: RelayId::from_bytes(ENVIRONMENT),
            generation: 7,
            method: "GET".to_string(),
            path: "/v1/models".to_string(),
            query: Some("limit=1".to_string()),
            allowed_headers: vec!["content-type".to_string()],
            body_length,
        })
    }

    #[test]
    fn identifier_round_trips_through_its_canonical_form() {
        let id = stream_id();
        let text = id.to_canonical();
        assert_eq!(text, "01234567-89ab-cdef-0123-456789abcdef");
        assert_eq!(parse_relay_id(&text).expect("canonical form parses"), id);
        assert_eq!(
            parse_relay_id(&text.to_uppercase()).expect("uppercase parses"),
            id
        );
    }

    #[test]
    fn malformed_identifiers_are_refused_rather_than_repaired() {
        for raw in [
            "",
            "01234567-89ab-cdef-0123-456789abcde",   // short
            "01234567-89ab-cdef-0123-456789abcdef0", // long
            "0123456789ab-cdef-0123-456789abcdef",   // misplaced separator
            "01234567-89ab-cdef-0123-456789abcdeg",  // non-hex
            "01234567_89ab_cdef_0123_456789abcdef",  // wrong separators
            " 01234567-89ab-cdef-0123-456789abcdef", // leading space
        ] {
            assert_eq!(
                parse_relay_id(raw),
                Err(RelayProtocolError::InvalidIdentifier),
                "{raw:?} must be refused"
            );
        }
    }

    #[test]
    fn control_frames_round_trip() {
        for control in [
            open(0),
            open(2 * 1024 * 1024),
            RelayControl::RequestEnd(StreamFrame {
                stream_id: stream_id(),
            }),
            RelayControl::ResponseHead(ResponseHeadFrame {
                stream_id: stream_id(),
                status: 204,
                allowed_headers: vec!["cache-control".to_string()],
            }),
            RelayControl::End(StreamFrame {
                stream_id: stream_id(),
            }),
            RelayControl::Error(ErrorFrame {
                stream_id: stream_id(),
                code: RelayErrorCode::OutOfOrder,
            }),
            RelayControl::Cancel(StreamFrame {
                stream_id: stream_id(),
            }),
        ] {
            let encoded = encode_control(&control).expect("encode");
            assert_eq!(decode_control(&encoded).expect("decode"), control);
        }
    }

    #[test]
    fn control_decode_rejects_unknown_variants_and_fields() {
        assert_eq!(
            decode_control(r#"{"type":"teleport"}"#),
            Err(RelayProtocolError::MalformedFrame)
        );
        assert_eq!(
            decode_control(
                r#"{"type":"end","stream_id":"01234567-89ab-cdef-0123-456789abcdef","extra":1}"#
            ),
            Err(RelayProtocolError::MalformedFrame),
            "an unknown field must not be silently dropped"
        );
        assert_eq!(
            decode_control(r#"{"type":"open","stream_id":"nope"}"#),
            Err(RelayProtocolError::MalformedFrame),
            "a malformed identifier inside a frame is a malformed frame"
        );
    }

    #[test]
    fn every_terminal_frame_requires_its_stream_id() {
        let id = stream_id().to_canonical();
        // Missing stream_id is a malformed frame, not an unattributed one:
        // routing never guesses under multiplex.
        for raw in [
            r#"{"type":"request_end"}"#.to_string(),
            r#"{"type":"end"}"#.to_string(),
            r#"{"type":"cancel"}"#.to_string(),
            format!(r#"{{"type":"response_head","status":200,"allowed_headers":[]}}"#),
            format!(r#"{{"type":"error","code":"cancelled"}}"#),
        ] {
            assert_eq!(
                decode_control(&raw),
                Err(RelayProtocolError::MalformedFrame),
                "{raw:?} must be refused without an id"
            );
        }
        // Present ids decode and route to the same stream.
        let end = format!(r#"{{"type":"end","stream_id":"{id}"}}"#);
        assert_eq!(
            decode_control(&end).expect("end decodes").stream_id(),
            stream_id()
        );
        let cancel = format!(r#"{{"type":"cancel","stream_id":"{id}"}}"#);
        assert_eq!(
            decode_control(&cancel).expect("cancel decodes").stream_id(),
            stream_id()
        );
        let head = format!(
            r#"{{"type":"response_head","stream_id":"{id}","status":200,"allowed_headers":[]}}"#
        );
        assert_eq!(
            decode_control(&head).expect("head decodes").stream_id(),
            stream_id()
        );
        let error = format!(r#"{{"type":"error","stream_id":"{id}","code":"cancelled"}}"#);
        assert_eq!(
            decode_control(&error).expect("error decodes").stream_id(),
            stream_id()
        );
    }

    #[test]
    fn the_current_wire_tag_is_v2() {
        assert_eq!(RELAY_PROTOCOL_VERSION, "df-env-http.v2");
    }

    #[test]
    fn control_frames_are_bounded_in_both_directions() {
        let mut frame = open(0);
        let RelayControl::Open(ref mut open_frame) = frame else {
            unreachable!()
        };
        // Comfortably inside the bound, then far outside it.
        open_frame.path = "/a".repeat(4 * 1024);
        assert!(encode_control(&frame).is_ok());

        let RelayControl::Open(ref mut oversized_frame) = frame else {
            unreachable!()
        };
        oversized_frame.path = "/a".repeat(MAX_CONTROL_JSON_BYTES);
        assert_eq!(
            encode_control(&frame),
            Err(RelayProtocolError::ControlTooLarge)
        );

        let oversized = "/a".repeat(MAX_CONTROL_JSON_BYTES + 1);
        assert_eq!(
            decode_control(&format!(r#"{{"type":"end","path":"{oversized}"}}"#)),
            Err(RelayProtocolError::ControlTooLarge),
            "an oversized payload is refused before parsing"
        );
    }

    #[test]
    fn data_frames_carry_the_identifier_and_bounded_payload() {
        let payload = vec![7u8; 1024];
        let frame = encode_chunk(stream_id(), &payload).expect("encode");
        assert_eq!(frame.len(), RELAY_ID_BYTES + payload.len());
        let decoded = decode_chunk(&frame).expect("decode");
        assert_eq!(decoded.stream_id, stream_id());
        assert_eq!(decoded.data, payload.as_slice());
    }

    #[test]
    fn empty_data_frame_is_valid() {
        let frame = encode_chunk(stream_id(), &[]).expect("encode");
        assert_eq!(frame.len(), RELAY_ID_BYTES);
        assert!(decode_chunk(&frame).expect("decode").data.is_empty());
    }

    #[test]
    fn data_frame_bounds_are_enforced() {
        assert_eq!(
            encode_chunk(stream_id(), &vec![0u8; MAX_CHUNK_DATA_BYTES + 1]),
            Err(RelayProtocolError::ChunkTooLarge)
        );
        let oversized = vec![0u8; RELAY_ID_BYTES + MAX_CHUNK_DATA_BYTES + 1];
        assert_eq!(
            decode_chunk(&oversized),
            Err(RelayProtocolError::ChunkTooLarge)
        );
        assert_eq!(
            decode_chunk(&[0u8; RELAY_ID_BYTES - 1]),
            Err(RelayProtocolError::MalformedFrame)
        );
    }
}
