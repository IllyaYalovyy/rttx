//! V3 chunked scrollback: builders, validation, and capability gating.
//!
//! Implements RFC-021 Section 8 (`OPT_CHUNKED_SCROLLBACK`).
//!
//! `GetScrollback` is a request/response command (uses `request_id`).
//! Each request returns a single `ScrollbackChunk`. The client pages by
//! adjusting `offset`. The server caps `limit` to [`MAX_CHUNK_SIZE`].
//!
//! Scrollback is append-only — already-fetched pages remain valid as new
//! output arrives. Without `OPT_CHUNKED_SCROLLBACK`, the client only has
//! `scrollback_tail` from the attach snapshot.

use crate::v3;

/// Maximum bytes the server will return in a single `ScrollbackChunk` (256 KB).
pub const MAX_CHUNK_SIZE: u32 = 256 * 1024;

/// Check whether `OPT_CHUNKED_SCROLLBACK` is in the effective capability set.
#[must_use]
pub fn is_supported(effective_caps: &[i32]) -> bool {
    effective_caps.contains(&(v3::Capability::OptChunkedScrollback as i32))
}

/// Cap a client-requested `limit` to [`MAX_CHUNK_SIZE`].
#[must_use]
pub fn cap_limit(requested: u32) -> u32 {
    requested.min(MAX_CHUNK_SIZE)
}

/// Build a `GetScrollback` request.
#[must_use]
pub fn build_get_scrollback(
    runtime_id: uuid::Uuid,
    pane_id: uuid::Uuid,
    offset: u64,
    limit: u32,
) -> v3::GetScrollback {
    v3::GetScrollback {
        runtime_id: crate::uuid_to_bytes(runtime_id),
        pane_id: crate::uuid_to_bytes(pane_id),
        offset,
        limit,
    }
}

/// Build a `ClientEnvelope` for a `GetScrollback` request.
#[must_use]
pub fn build_get_scrollback_envelope(
    id_gen: &crate::v3_envelope::RequestIdGenerator,
    request: v3::GetScrollback,
) -> v3::ClientEnvelope {
    crate::v3_envelope::build_client_envelope(
        id_gen,
        v3::client_envelope::Command::GetScrollback(request),
    )
}

/// Build a `ScrollbackChunk` response.
#[must_use]
pub fn build_scrollback_chunk(
    runtime_id: uuid::Uuid,
    pane_id: uuid::Uuid,
    offset: u64,
    data: bytes::Bytes,
    is_last: bool,
) -> v3::ScrollbackChunk {
    v3::ScrollbackChunk {
        runtime_id: crate::uuid_to_bytes(runtime_id),
        pane_id: crate::uuid_to_bytes(pane_id),
        offset,
        data,
        is_last,
    }
}

/// Build a `ServerEnvelope` response containing a `ScrollbackChunk`.
#[must_use]
pub fn build_scrollback_chunk_response(
    request_id: u64,
    chunk: v3::ScrollbackChunk,
) -> v3::ServerEnvelope {
    crate::v3_envelope::build_response_envelope(
        request_id,
        v3::server_envelope::Payload::ScrollbackChunk(chunk),
    )
}

/// Slice scrollback data for a `GetScrollback` request.
///
/// Given the full scrollback buffer, the requested `offset`, and the
/// server-capped `limit`, returns `(data, is_last)`.
///
/// - If `offset >= total`, returns empty data with `is_last = true`.
/// - Otherwise returns up to `limit` bytes starting at `offset`.
/// - `is_last` is true when the chunk reaches the end of the buffer.
#[must_use]
pub fn slice_scrollback(scrollback: &[u8], offset: u64, limit: u32) -> (bytes::Bytes, bool) {
    let total = scrollback.len() as u64;
    if offset >= total {
        return (bytes::Bytes::new(), true);
    }
    let start = offset as usize;
    let end = (start + limit as usize).min(scrollback.len());
    let is_last = end as u64 >= total;
    (bytes::Bytes::copy_from_slice(&scrollback[start..end]), is_last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode_frame, encode_frame, uuid_to_bytes, v3_envelope::RequestIdGenerator};
    use bytes::BytesMut;

    fn rt() -> uuid::Uuid {
        uuid::Uuid::new_v4()
    }

    fn pn() -> uuid::Uuid {
        uuid::Uuid::new_v4()
    }

    // ── MAX_CHUNK_SIZE ──

    #[test]
    fn max_chunk_size_is_256kb() {
        assert_eq!(MAX_CHUNK_SIZE, 256 * 1024);
    }

    // ── is_supported ──

    #[test]
    fn supported_when_capability_present() {
        let caps = vec![
            v3::Capability::CoreRuntimeLifecycle as i32,
            v3::Capability::OptChunkedScrollback as i32,
        ];
        assert!(is_supported(&caps));
    }

    #[test]
    fn not_supported_when_capability_absent() {
        let caps = vec![
            v3::Capability::CoreRuntimeLifecycle as i32,
            v3::Capability::OptDiagnostics as i32,
        ];
        assert!(!is_supported(&caps));
    }

    #[test]
    fn not_supported_with_empty_caps() {
        assert!(!is_supported(&[]));
    }

    // ── cap_limit ──

    #[test]
    fn cap_limit_passes_through_small_value() {
        assert_eq!(cap_limit(1024), 1024);
    }

    #[test]
    fn cap_limit_clamps_to_max() {
        assert_eq!(cap_limit(MAX_CHUNK_SIZE + 1), MAX_CHUNK_SIZE);
    }

    #[test]
    fn cap_limit_exact_max_passes_through() {
        assert_eq!(cap_limit(MAX_CHUNK_SIZE), MAX_CHUNK_SIZE);
    }

    #[test]
    fn cap_limit_zero() {
        assert_eq!(cap_limit(0), 0);
    }

    #[test]
    fn cap_limit_u32_max() {
        assert_eq!(cap_limit(u32::MAX), MAX_CHUNK_SIZE);
    }

    // ── build_get_scrollback ──

    #[test]
    fn get_scrollback_populates_all_fields() {
        let r = rt();
        let p = pn();
        let req = build_get_scrollback(r, p, 4096, 65536);
        assert_eq!(req.runtime_id, uuid_to_bytes(r));
        assert_eq!(req.pane_id, uuid_to_bytes(p));
        assert_eq!(req.offset, 4096);
        assert_eq!(req.limit, 65536);
    }

    #[test]
    fn get_scrollback_wire_roundtrip() {
        let req = build_get_scrollback(rt(), pn(), 0, MAX_CHUNK_SIZE);
        let mut buf = BytesMut::new();
        encode_frame(&req, &mut buf).unwrap();
        let decoded: v3::GetScrollback = decode_frame(&mut buf).unwrap();
        assert_eq!(req, decoded);
    }

    // ── build_get_scrollback_envelope ──

    #[test]
    fn get_scrollback_envelope_has_nonzero_request_id() {
        let id_gen = RequestIdGenerator::new();
        let req = build_get_scrollback(rt(), pn(), 0, 65536);
        let env = build_get_scrollback_envelope(&id_gen, req);
        assert_ne!(env.request_id, 0);
    }

    #[test]
    fn get_scrollback_envelope_contains_correct_command() {
        let id_gen = RequestIdGenerator::new();
        let r = rt();
        let p = pn();
        let req = build_get_scrollback(r, p, 100, 200);
        let env = build_get_scrollback_envelope(&id_gen, req);
        match env.command {
            Some(v3::client_envelope::Command::GetScrollback(ref gs)) => {
                assert_eq!(gs.runtime_id, uuid_to_bytes(r));
                assert_eq!(gs.pane_id, uuid_to_bytes(p));
                assert_eq!(gs.offset, 100);
                assert_eq!(gs.limit, 200);
            }
            _ => panic!("expected GetScrollback command"),
        }
    }

    #[test]
    fn get_scrollback_envelope_wire_roundtrip() {
        let id_gen = RequestIdGenerator::new();
        let req = build_get_scrollback(rt(), pn(), 0, 65536);
        let env = build_get_scrollback_envelope(&id_gen, req);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    // ── build_scrollback_chunk ──

    #[test]
    fn scrollback_chunk_populates_all_fields() {
        let r = rt();
        let p = pn();
        let chunk = build_scrollback_chunk(r, p, 4096, bytes::Bytes::from_static(b"data"), true);
        assert_eq!(chunk.runtime_id, uuid_to_bytes(r));
        assert_eq!(chunk.pane_id, uuid_to_bytes(p));
        assert_eq!(chunk.offset, 4096);
        assert_eq!(chunk.data.as_ref(), b"data");
        assert!(chunk.is_last);
    }

    #[test]
    fn scrollback_chunk_not_last() {
        let chunk =
            build_scrollback_chunk(rt(), pn(), 0, bytes::Bytes::from_static(b"partial"), false);
        assert!(!chunk.is_last);
    }

    #[test]
    fn scrollback_chunk_wire_roundtrip() {
        let chunk = build_scrollback_chunk(
            rt(),
            pn(),
            1024,
            bytes::Bytes::from_static(b"scrollback content"),
            false,
        );
        let mut buf = BytesMut::new();
        encode_frame(&chunk, &mut buf).unwrap();
        let decoded: v3::ScrollbackChunk = decode_frame(&mut buf).unwrap();
        assert_eq!(chunk, decoded);
    }

    // ── build_scrollback_chunk_response ──

    #[test]
    fn chunk_response_echoes_request_id() {
        let chunk = build_scrollback_chunk(rt(), pn(), 0, bytes::Bytes::from_static(b"data"), true);
        let env = build_scrollback_chunk_response(42, chunk.clone());
        assert_eq!(env.request_id, 42);
        match env.payload {
            Some(v3::server_envelope::Payload::ScrollbackChunk(ref c)) => {
                assert_eq!(c, &chunk);
            }
            _ => panic!("expected ScrollbackChunk payload"),
        }
    }

    #[test]
    fn chunk_response_is_not_push_event() {
        let chunk = build_scrollback_chunk(rt(), pn(), 0, bytes::Bytes::from_static(b"data"), true);
        let env = build_scrollback_chunk_response(1, chunk);
        assert!(!crate::v3_envelope::is_push_event(&env));
    }

    #[test]
    fn chunk_response_wire_roundtrip() {
        let chunk = build_scrollback_chunk(
            rt(),
            pn(),
            512,
            bytes::Bytes::from_static(b"chunk data"),
            false,
        );
        let env = build_scrollback_chunk_response(99, chunk);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    // ── slice_scrollback ──

    #[test]
    fn slice_from_start_within_buffer() {
        let data = b"hello world";
        let (chunk, is_last) = slice_scrollback(data, 0, 5);
        assert_eq!(chunk.as_ref(), b"hello");
        assert!(!is_last);
    }

    #[test]
    fn slice_from_start_entire_buffer() {
        let data = b"hello";
        let (chunk, is_last) = slice_scrollback(data, 0, 100);
        assert_eq!(chunk.as_ref(), b"hello");
        assert!(is_last);
    }

    #[test]
    fn slice_from_middle() {
        let data = b"abcdefghij";
        let (chunk, is_last) = slice_scrollback(data, 3, 4);
        assert_eq!(chunk.as_ref(), b"defg");
        assert!(!is_last);
    }

    #[test]
    fn slice_to_end() {
        let data = b"abcdefghij";
        let (chunk, is_last) = slice_scrollback(data, 7, 100);
        assert_eq!(chunk.as_ref(), b"hij");
        assert!(is_last);
    }

    #[test]
    fn slice_exact_end() {
        let data = b"abcde";
        let (chunk, is_last) = slice_scrollback(data, 3, 2);
        assert_eq!(chunk.as_ref(), b"de");
        assert!(is_last);
    }

    #[test]
    fn slice_offset_at_end() {
        let data = b"hello";
        let (chunk, is_last) = slice_scrollback(data, 5, 100);
        assert!(chunk.is_empty());
        assert!(is_last);
    }

    #[test]
    fn slice_offset_beyond_end() {
        let data = b"hello";
        let (chunk, is_last) = slice_scrollback(data, 1000, 100);
        assert!(chunk.is_empty());
        assert!(is_last);
    }

    #[test]
    fn slice_empty_buffer() {
        let (chunk, is_last) = slice_scrollback(b"", 0, 100);
        assert!(chunk.is_empty());
        assert!(is_last);
    }

    #[test]
    fn slice_zero_limit() {
        let data = b"hello";
        let (chunk, is_last) = slice_scrollback(data, 0, 0);
        assert!(chunk.is_empty());
        assert!(!is_last);
    }

    #[test]
    fn slice_single_byte() {
        let data = b"x";
        let (chunk, is_last) = slice_scrollback(data, 0, 1);
        assert_eq!(chunk.as_ref(), b"x");
        assert!(is_last);
    }

    // ── Integration: paging through scrollback ──

    #[test]
    fn paging_retrieves_full_scrollback() {
        let scrollback = b"0123456789abcdef";
        let page_size: u32 = 4;
        let mut offset: u64 = 0;
        let mut collected = Vec::new();

        loop {
            let (chunk, is_last) = slice_scrollback(scrollback, offset, page_size);
            collected.extend_from_slice(&chunk);
            offset += chunk.len() as u64;
            if is_last {
                break;
            }
        }

        assert_eq!(collected, scrollback);
    }

    #[test]
    fn paging_with_capped_limit() {
        let scrollback = vec![b'x'; 1_000_000];
        let mut offset: u64 = 0;
        let mut total_bytes = 0_usize;
        let mut page_count = 0_u32;

        loop {
            let capped = cap_limit(MAX_CHUNK_SIZE);
            let (chunk, is_last) = slice_scrollback(&scrollback, offset, capped);
            total_bytes += chunk.len();
            offset += chunk.len() as u64;
            page_count += 1;
            if is_last {
                break;
            }
        }

        assert_eq!(total_bytes, scrollback.len());
        // 1 MB / 256 KB = ~4 pages
        assert!(page_count >= 4);
    }

    // ── Integration: error response for unsupported capability ──

    #[test]
    fn unsupported_capability_error_for_scrollback() {
        let err = crate::v3_error::build_error(
            v3::ErrorKind::UnsupportedCapability,
            "OPT_CHUNKED_SCROLLBACK not negotiated",
            "GetScrollback",
        );
        let env = crate::v3_error::build_error_response(42, err);
        assert_eq!(env.request_id, 42);
        match env.payload {
            Some(v3::server_envelope::Payload::Error(ref e)) => {
                assert_eq!(e.kind, v3::ErrorKind::UnsupportedCapability as i32);
                assert_eq!(e.operation, "GetScrollback");
            }
            _ => panic!("expected Error payload"),
        }
    }
}
