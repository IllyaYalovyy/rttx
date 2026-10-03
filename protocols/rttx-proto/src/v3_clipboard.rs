//! V3 clipboard: capability gating and the OSC 52 clipboard-write push.
//!
//! An application running in a pane puts text on the system clipboard by
//! writing OSC 52. The daemon owns the PTY, so it decodes the payload,
//! strips the sequence from the client-bound stream, and pushes a
//! `ClipboardWrite` instead. This event is the only way a client learns
//! about such a write.
//!
//! The push is gated on `OPT_CLIPBOARD_OSC52` so a client built before the
//! event existed never receives an unknown payload.
//!
//! Write-only by design: the OSC 52 read/query form is never implemented,
//! because answering it would let any process that can print to a pane —
//! including one on a remote host — read the user's clipboard.

use crate::v3;

/// Check whether `OPT_CLIPBOARD_OSC52` is in the effective capability set.
#[must_use]
pub fn is_supported(effective_caps: &[i32]) -> bool {
    effective_caps.contains(&(v3::Capability::OptClipboardOsc52 as i32))
}

/// Build a `ClipboardWrite` push event.
#[must_use]
pub fn build_clipboard_write(
    runtime_id: uuid::Uuid,
    pane_id: uuid::Uuid,
    target: &str,
    data: bytes::Bytes,
) -> v3::ClipboardWrite {
    v3::ClipboardWrite {
        runtime_id: crate::uuid_to_bytes(runtime_id),
        pane_id: crate::uuid_to_bytes(pane_id),
        target: target.into(),
        data,
    }
}

/// Build a `ServerEnvelope` push carrying a `ClipboardWrite`.
#[must_use]
pub fn build_clipboard_write_push(write: v3::ClipboardWrite) -> v3::ServerEnvelope {
    crate::v3_envelope::build_push_envelope(v3::server_envelope::Payload::ClipboardWrite(write))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_gating_requires_the_clipboard_capability() {
        assert!(!is_supported(&[]));
        assert!(!is_supported(&[v3::Capability::OptDiagnostics as i32]));
        assert!(is_supported(&[
            v3::Capability::OptDiagnostics as i32,
            v3::Capability::OptClipboardOsc52 as i32,
        ]));
    }

    #[test]
    fn clipboard_write_push_round_trips_through_a_frame() {
        let runtime_id = uuid::Uuid::new_v4();
        let pane_id = uuid::Uuid::new_v4();
        let push = build_clipboard_write_push(build_clipboard_write(
            runtime_id,
            pane_id,
            "c",
            bytes::Bytes::from_static("héllo".as_bytes()),
        ));
        assert_eq!(push.request_id, 0, "clipboard writes are pushes, not responses");

        let mut buf = bytes::BytesMut::new();
        crate::encode_frame(&push, &mut buf).expect("encode");
        let decoded: v3::ServerEnvelope = crate::decode_frame(&mut buf).expect("decode");
        let Some(v3::server_envelope::Payload::ClipboardWrite(write)) = decoded.payload else {
            panic!("expected ClipboardWrite");
        };
        assert_eq!(write.runtime_id, runtime_id.as_bytes().to_vec());
        assert_eq!(write.pane_id, pane_id.as_bytes().to_vec());
        assert_eq!(write.target, "c");
        assert_eq!(write.data, bytes::Bytes::from_static("héllo".as_bytes()));
    }
}
