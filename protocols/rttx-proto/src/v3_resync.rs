//! V3 resync: capability gating, builders, and overflow handling.
//!
//! Implements RFC-021 Section 8 (`OPT_RESYNC`).
//!
//! When the server's bounded push channel drops messages for a client, it
//! sends `StreamOverflow`. The client requests `ResyncWorkspace`, and the
//! server responds with a fresh `WorkspaceSnapshot`.
//!
//! Without `OPT_RESYNC`, the server forcibly disconnects the client on
//! overflow. The client's connection state machine reconnects and receives
//! a fresh snapshot on reattach.

use crate::v3;

/// Check whether `OPT_RESYNC` is in the effective capability set.
#[must_use]
pub fn is_supported(effective_caps: &[i32]) -> bool {
    effective_caps.contains(&(v3::Capability::OptResync as i32))
}

/// Build a `StreamOverflow` push event.
///
/// `pane_id` is `Some` for pane-level overflow, `None` for workspace-level.
#[must_use]
pub fn build_stream_overflow(
    runtime_id: uuid::Uuid,
    pane_id: Option<uuid::Uuid>,
    dropped_count: u64,
) -> v3::StreamOverflow {
    v3::StreamOverflow {
        runtime_id: crate::uuid_to_bytes(runtime_id),
        pane_id: pane_id.map(crate::uuid_to_bytes),
        dropped_count,
    }
}

/// Build a `ServerEnvelope` push event containing a `StreamOverflow`.
#[must_use]
pub fn build_stream_overflow_envelope(overflow: v3::StreamOverflow) -> v3::ServerEnvelope {
    crate::v3_envelope::build_push_envelope(v3::server_envelope::Payload::StreamOverflow(overflow))
}

/// Build a `ResyncWorkspace` request.
#[must_use]
pub fn build_resync_workspace(runtime_id: uuid::Uuid) -> v3::ResyncWorkspace {
    v3::ResyncWorkspace { runtime_id: crate::uuid_to_bytes(runtime_id) }
}

/// Build a `ClientEnvelope` for a `ResyncWorkspace` request.
#[must_use]
pub fn build_resync_workspace_envelope(
    id_gen: &crate::v3_envelope::RequestIdGenerator,
    request: v3::ResyncWorkspace,
) -> v3::ClientEnvelope {
    crate::v3_envelope::build_client_envelope(
        id_gen,
        v3::client_envelope::Command::ResyncWorkspace(request),
    )
}

/// Build a `ServerEnvelope` response containing a `WorkspaceSnapshot` for resync.
///
/// This reuses the snapshot response builder — the wire format is identical
/// to an attach snapshot response.
#[must_use]
pub fn build_resync_response(request_id: u64, snapshot: v3::WorkspaceSnapshot) -> v3::ServerEnvelope {
    crate::v3_snapshot::build_snapshot_response(request_id, snapshot)
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

    // ── is_supported ──

    #[test]
    fn supported_when_capability_present() {
        let caps =
            vec![v3::Capability::CoreWorkspaceLifecycle as i32, v3::Capability::OptResync as i32];
        assert!(is_supported(&caps));
    }

    #[test]
    fn not_supported_when_capability_absent() {
        let caps = vec![
            v3::Capability::CoreWorkspaceLifecycle as i32,
            v3::Capability::OptDiagnostics as i32,
        ];
        assert!(!is_supported(&caps));
    }

    #[test]
    fn not_supported_with_empty_caps() {
        assert!(!is_supported(&[]));
    }

    // ── build_stream_overflow ──

    #[test]
    fn stream_overflow_with_pane_id() {
        let r = rt();
        let p = pn();
        let overflow = build_stream_overflow(r, Some(p), 42);
        assert_eq!(overflow.runtime_id, uuid_to_bytes(r));
        assert_eq!(overflow.pane_id, Some(uuid_to_bytes(p)));
        assert_eq!(overflow.dropped_count, 42);
    }

    #[test]
    fn stream_overflow_without_pane_id() {
        let r = rt();
        let overflow = build_stream_overflow(r, None, 10);
        assert_eq!(overflow.runtime_id, uuid_to_bytes(r));
        assert_eq!(overflow.pane_id, None);
        assert_eq!(overflow.dropped_count, 10);
    }

    #[test]
    fn stream_overflow_zero_dropped() {
        let overflow = build_stream_overflow(rt(), None, 0);
        assert_eq!(overflow.dropped_count, 0);
    }

    #[test]
    fn stream_overflow_wire_roundtrip() {
        for pane_id in [Some(pn()), None] {
            let overflow = build_stream_overflow(rt(), pane_id, 100);
            let mut buf = BytesMut::new();
            encode_frame(&overflow, &mut buf).unwrap();
            let decoded: v3::StreamOverflow = decode_frame(&mut buf).unwrap();
            assert_eq!(overflow, decoded);
        }
    }

    // ── build_stream_overflow_envelope ──

    #[test]
    fn stream_overflow_envelope_is_push_event() {
        let overflow = build_stream_overflow(rt(), Some(pn()), 5);
        let env = build_stream_overflow_envelope(overflow);
        assert_eq!(env.request_id, 0);
        assert!(crate::v3_envelope::is_push_event(&env));
    }

    #[test]
    fn stream_overflow_envelope_contains_correct_payload() {
        let r = rt();
        let p = pn();
        let overflow = build_stream_overflow(r, Some(p), 7);
        let env = build_stream_overflow_envelope(overflow);
        match env.payload {
            Some(v3::server_envelope::Payload::StreamOverflow(ref so)) => {
                assert_eq!(so.runtime_id, uuid_to_bytes(r));
                assert_eq!(so.pane_id, Some(uuid_to_bytes(p)));
                assert_eq!(so.dropped_count, 7);
            }
            _ => panic!("expected StreamOverflow payload"),
        }
    }

    #[test]
    fn stream_overflow_envelope_wire_roundtrip() {
        let overflow = build_stream_overflow(rt(), None, 50);
        let env = build_stream_overflow_envelope(overflow);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    // ── build_resync_workspace ──

    #[test]
    fn resync_workspace_populates_runtime_id() {
        let r = rt();
        let req = build_resync_workspace(r);
        assert_eq!(req.runtime_id, uuid_to_bytes(r));
    }

    #[test]
    fn resync_workspace_wire_roundtrip() {
        let req = build_resync_workspace(rt());
        let mut buf = BytesMut::new();
        encode_frame(&req, &mut buf).unwrap();
        let decoded: v3::ResyncWorkspace = decode_frame(&mut buf).unwrap();
        assert_eq!(req, decoded);
    }

    // ── build_resync_workspace_envelope ──

    #[test]
    fn resync_envelope_has_nonzero_request_id() {
        let id_gen = RequestIdGenerator::new();
        let req = build_resync_workspace(rt());
        let env = build_resync_workspace_envelope(&id_gen, req);
        assert_ne!(env.request_id, 0);
    }

    #[test]
    fn resync_envelope_contains_correct_command() {
        let id_gen = RequestIdGenerator::new();
        let r = rt();
        let req = build_resync_workspace(r);
        let env = build_resync_workspace_envelope(&id_gen, req);
        match env.command {
            Some(v3::client_envelope::Command::ResyncWorkspace(ref rs)) => {
                assert_eq!(rs.runtime_id, uuid_to_bytes(r));
            }
            _ => panic!("expected ResyncWorkspace command"),
        }
    }

    #[test]
    fn resync_envelope_wire_roundtrip() {
        let id_gen = RequestIdGenerator::new();
        let req = build_resync_workspace(rt());
        let env = build_resync_workspace_envelope(&id_gen, req);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    // ── build_resync_response ──

    #[test]
    fn resync_response_echoes_request_id() {
        let snap = crate::v3_snapshot::build_workspace_snapshot(
            rt(),
            42,
            v3::WorkspaceClientRole::Writer,
            vec![],
        );
        let env = build_resync_response(7, snap.clone());
        assert_eq!(env.request_id, 7);
        match env.payload {
            Some(v3::server_envelope::Payload::WorkspaceSnapshot(ref s)) => {
                assert_eq!(s, &snap);
            }
            _ => panic!("expected WorkspaceSnapshot payload"),
        }
    }

    #[test]
    fn resync_response_is_not_push_event() {
        let snap = crate::v3_snapshot::build_workspace_snapshot(
            rt(),
            1,
            v3::WorkspaceClientRole::Writer,
            vec![],
        );
        let env = build_resync_response(42, snap);
        assert!(!crate::v3_envelope::is_push_event(&env));
    }

    #[test]
    fn resync_response_wire_roundtrip() {
        let pane =
            crate::v3_snapshot::build_pane_snapshot(crate::v3_snapshot::PaneSnapshotParams {
                pane_id: pn(),
                pane_output_seq: 50,
                title: "bash".into(),
                cwd: "/home".into(),
                cols: 80,
                rows: 24,
                exit_status: None,
                terminal_modes: v3::TerminalModeState::default(),
                scrollback_tail: bytes::Bytes::from_static(b"$ "),
                total_scrollback_bytes: 2,
            });
        let snap = crate::v3_snapshot::build_workspace_snapshot(
            rt(),
            99,
            v3::WorkspaceClientRole::Writer,
            vec![pane],
        );
        let env = build_resync_response(55, snap);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    // ── Integration: unsupported capability error ──

    #[test]
    fn unsupported_capability_error_for_resync() {
        let err = crate::v3_error::build_error(
            v3::ErrorKind::UnsupportedCapability,
            "OPT_RESYNC not negotiated",
            "ResyncWorkspace",
        );
        let env = crate::v3_error::build_error_response(42, err);
        assert_eq!(env.request_id, 42);
        match env.payload {
            Some(v3::server_envelope::Payload::Error(ref e)) => {
                assert_eq!(e.kind, v3::ErrorKind::UnsupportedCapability as i32);
                assert_eq!(e.operation, "ResyncWorkspace");
            }
            _ => panic!("expected Error payload"),
        }
    }

    // ── Integration: overflow → resync → snapshot flow ──

    #[test]
    fn overflow_triggers_resync_flow() {
        let id_gen = RequestIdGenerator::new();
        let r = rt();
        let p = pn();

        // 1. Server detects overflow and sends StreamOverflow push event
        let overflow = build_stream_overflow(r, Some(p), 3);
        let overflow_env = build_stream_overflow_envelope(overflow);
        assert!(crate::v3_envelope::is_push_event(&overflow_env));

        // 2. Client receives overflow and sends ResyncWorkspace request
        let resync_req = build_resync_workspace(r);
        let resync_env = build_resync_workspace_envelope(&id_gen, resync_req);
        let saved_request_id = resync_env.request_id;
        assert_ne!(saved_request_id, 0);

        // 3. Server responds with fresh WorkspaceSnapshot
        let pane =
            crate::v3_snapshot::build_pane_snapshot(crate::v3_snapshot::PaneSnapshotParams {
                pane_id: p,
                pane_output_seq: 100,
                title: "bash".into(),
                cwd: "/home".into(),
                cols: 80,
                rows: 24,
                exit_status: None,
                terminal_modes: v3::TerminalModeState::default(),
                scrollback_tail: bytes::Bytes::from_static(b"$ "),
                total_scrollback_bytes: 2,
            });
        let snap = crate::v3_snapshot::build_workspace_snapshot(
            r,
            50,
            v3::WorkspaceClientRole::Writer,
            vec![pane],
        );
        let snap_env = build_resync_response(saved_request_id, snap);
        assert_eq!(snap_env.request_id, saved_request_id);
        assert!(!crate::v3_envelope::is_push_event(&snap_env));
    }

    // ── Integration: gap detection triggers resync as fallback ──

    #[test]
    fn seq_gap_detection_as_resync_fallback() {
        let id_gen = RequestIdGenerator::new();
        let r = rt();

        // Client detects a gap in pane_output_seq
        let gap = crate::v3_snapshot::detect_output_seq_gap(5, 8);
        assert_eq!(gap, Some(3));

        // Client has OPT_RESYNC, so it sends ResyncWorkspace
        let caps =
            vec![v3::Capability::CoreWorkspaceLifecycle as i32, v3::Capability::OptResync as i32];
        assert!(is_supported(&caps));

        let req = build_resync_workspace(r);
        let env = build_resync_workspace_envelope(&id_gen, req);
        assert_ne!(env.request_id, 0);
        match env.command {
            Some(v3::client_envelope::Command::ResyncWorkspace(ref rs)) => {
                assert_eq!(rs.runtime_id, uuid_to_bytes(r));
            }
            _ => panic!("expected ResyncWorkspace command"),
        }
    }

    // ── Integration: without OPT_RESYNC, server sends disconnect error ──

    #[test]
    fn without_resync_server_disconnects_on_overflow() {
        let caps = vec![v3::Capability::CoreWorkspaceLifecycle as i32];
        assert!(!is_supported(&caps));

        // Server builds a stream overflow error to disconnect the client
        let err = crate::v3_error::build_error(
            v3::ErrorKind::StreamOverflow,
            "push channel overflow; client does not support OPT_RESYNC",
            "push",
        );
        assert!(err.retryable);

        let env = crate::v3_error::build_error_response(0, err);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
        match decoded.payload {
            Some(v3::server_envelope::Payload::Error(ref e)) => {
                assert_eq!(e.kind, v3::ErrorKind::StreamOverflow as i32);
                assert!(e.retryable);
            }
            _ => panic!("expected Error payload"),
        }
    }
}
