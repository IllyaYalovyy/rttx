//! Property-based robustness tests for protocol framing and UUID parsing.
//!
//! Validates that `decode_frame` and `bytes_to_uuid` never panic on arbitrary
//! input, and that valid encode→decode round-trips are lossless.

use bytes::BytesMut;
use proptest::prelude::*;
use rttx_proto::{bytes_to_uuid, decode_frame, encode_frame, uuid_to_bytes, v3, MAX_MESSAGE_SIZE};

// ── Framing: arbitrary bytes never panic ────────────────────────────

proptest! {
    #[test]
    fn decode_frame_never_panics_on_arbitrary_bytes(data in proptest::collection::vec(any::<u8>(), 0..1024)) {
        let mut buf = BytesMut::from(data.as_slice());
        let _: Result<v3::ClientEnvelope, _> = decode_frame(&mut buf);
    }

    #[test]
    fn decode_frame_never_panics_on_arbitrary_server_bytes(data in proptest::collection::vec(any::<u8>(), 0..1024)) {
        let mut buf = BytesMut::from(data.as_slice());
        let _: Result<v3::ServerEnvelope, _> = decode_frame(&mut buf);
    }

    #[test]
    fn decode_frame_rejects_oversized_length_prefix(len in (MAX_MESSAGE_SIZE + 1)..=u32::MAX) {
        let mut buf = BytesMut::new();
        buf.extend_from_slice(&len.to_le_bytes());
        buf.extend_from_slice(&[0u8; 64]);
        let result: Result<v3::ClientEnvelope, _> = decode_frame(&mut buf);
        prop_assert!(result.is_err());
    }
}

// ── Framing: valid encode→decode round-trip ─────────────────────────

proptest! {
    #[test]
    fn ping_roundtrip(nonce in any::<u64>()) {
        let msg = v3::ClientEnvelope {
            request_id: nonce,
            command: Some(v3::client_envelope::Command::Ping(v3::Ping { nonce })),
        };
        let mut buf = BytesMut::new();
        encode_frame(&msg, &mut buf).unwrap();
        let decoded: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
        prop_assert_eq!(msg, decoded);
    }

    #[test]
    fn output_delta_roundtrip(
        data in proptest::collection::vec(any::<u8>(), 0..4096),
        seq in any::<u64>(),
    ) {
        let msg = v3::ServerEnvelope {
            request_id: 0,
            payload: Some(v3::server_envelope::Payload::OutputDelta(v3::OutputDelta {
                runtime_id: uuid_to_bytes(uuid::Uuid::new_v4()),
                pane_id: uuid_to_bytes(uuid::Uuid::new_v4()),
                data: bytes::Bytes::from(data),
                pane_output_seq: seq,
            })),
        };
        let mut buf = BytesMut::new();
        encode_frame(&msg, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
        prop_assert_eq!(msg, decoded);
    }
}

// ── Framing: partial frames return Incomplete ───────────────────────

proptest! {
    #[test]
    fn partial_frame_returns_incomplete(
        payload in proptest::collection::vec(any::<u8>(), 1..256),
        cut in 0..256usize,
    ) {
        let len = payload.len() as u32;
        let mut full = Vec::with_capacity(4 + payload.len());
        full.extend_from_slice(&len.to_le_bytes());
        full.extend_from_slice(&payload);

        let cut = cut.min(full.len().saturating_sub(1));
        let mut buf = BytesMut::from(&full[..cut]);
        let result: Result<v3::ClientEnvelope, _> = decode_frame(&mut buf);
        prop_assert!(result.is_err());
    }
}

// ── UUID: arbitrary bytes never panic ───────────────────────────────

proptest! {
    #[test]
    fn bytes_to_uuid_never_panics(data in proptest::collection::vec(any::<u8>(), 0..32)) {
        let _ = bytes_to_uuid(&data);
    }

    #[test]
    fn uuid_roundtrip_is_lossless(bytes in proptest::collection::vec(any::<u8>(), 16..=16)) {
        let arr: [u8; 16] = bytes.try_into().unwrap();
        let id = uuid::Uuid::from_bytes(arr);
        let encoded = uuid_to_bytes(id);
        let recovered = bytes_to_uuid(&encoded).unwrap();
        prop_assert_eq!(id, recovered);
    }

    #[test]
    fn bytes_to_uuid_rejects_wrong_length(data in proptest::collection::vec(any::<u8>(), 0..32).prop_filter("not 16 bytes", |v| v.len() != 16)) {
        prop_assert!(bytes_to_uuid(&data).is_err());
    }
}
