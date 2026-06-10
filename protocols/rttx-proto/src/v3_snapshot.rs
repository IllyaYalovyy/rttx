//! V3 snapshot and output delta: builders and helpers.
//!
//! Implements RFC-021 Sections 7–8 (Revision Semantics, Snapshots and Resync).
//!
//! `RuntimeSnapshot` is sent on successful attach and contains the full
//! runtime state including per-pane scrollback tails. `OutputDelta` carries
//! incremental terminal output with a per-pane monotonic `pane_output_seq`.
//!
//! Two revision spaces:
//! - `runtime_revision` (structural): bumped by pane create/close/exit,
//!   rename, mode change, client attach/detach.
//! - `pane_output_seq` (output continuity): per-pane counter incremented
//!   by 1 for every `OutputDelta`. Gaps indicate dropped messages.

use crate::v3;

/// Default scrollback tail limit (256 KB) as specified in RFC-021 Section 8.
pub const DEFAULT_SCROLLBACK_TAIL_LIMIT: usize = 256 * 1024;

/// Parameters for building a `PaneSnapshot`.
pub struct PaneSnapshotParams {
    pub pane_id: uuid::Uuid,
    pub pane_output_seq: u64,
    pub title: String,
    pub cwd: String,
    pub cols: u32,
    pub rows: u32,
    pub exit_status: Option<i32>,
    pub terminal_modes: v3::TerminalModeState,
    pub scrollback_tail: bytes::Bytes,
    pub total_scrollback_bytes: u64,
}

/// Build a `PaneSnapshot`.
///
/// `scrollback_complete` is derived automatically: `true` when
/// `scrollback_tail.len() >= total_scrollback_bytes`.
#[must_use]
pub fn build_pane_snapshot(params: PaneSnapshotParams) -> v3::PaneSnapshot {
    let scrollback_complete = params.scrollback_tail.len() as u64 >= params.total_scrollback_bytes;
    v3::PaneSnapshot {
        pane_id: crate::uuid_to_bytes(params.pane_id),
        pane_output_seq: params.pane_output_seq,
        title: params.title,
        cwd: params.cwd,
        cols: params.cols,
        rows: params.rows,
        exit_status: params.exit_status,
        terminal_modes: Some(params.terminal_modes),
        scrollback_tail: params.scrollback_tail,
        total_scrollback_bytes: params.total_scrollback_bytes,
        scrollback_complete,
    }
}

/// Build a `RuntimeSnapshot` response for a successful attach.
///
/// The workspace tree is left empty; use [`build_runtime_snapshot_with_tree`]
/// to carry the authoritative structure (RFC-031).
#[must_use]
pub fn build_runtime_snapshot(
    runtime_id: uuid::Uuid,
    runtime_revision: u64,
    client_role: v3::RuntimeClientRole,
    panes: Vec<v3::PaneSnapshot>,
) -> v3::RuntimeSnapshot {
    v3::RuntimeSnapshot {
        runtime_id: crate::uuid_to_bytes(runtime_id),
        runtime_revision,
        client_role: client_role as i32,
        panes,
        tree: None,
        default_active_pane_id: Vec::new(),
    }
}

/// Build a `RuntimeSnapshot` carrying the authoritative workspace tree and
/// fallback-focus pane (RFC-031 §5).
#[must_use]
pub fn build_runtime_snapshot_with_tree(
    runtime_id: uuid::Uuid,
    runtime_revision: u64,
    client_role: v3::RuntimeClientRole,
    panes: Vec<v3::PaneSnapshot>,
    tree: Option<v3::PaneTreeNode>,
    default_active_pane_id: Vec<u8>,
) -> v3::RuntimeSnapshot {
    v3::RuntimeSnapshot {
        runtime_id: crate::uuid_to_bytes(runtime_id),
        runtime_revision,
        client_role: client_role as i32,
        panes,
        tree,
        default_active_pane_id,
    }
}

/// Build a `ServerEnvelope` response containing a `RuntimeSnapshot`.
#[must_use]
pub fn build_snapshot_response(
    request_id: u64,
    snapshot: v3::RuntimeSnapshot,
) -> v3::ServerEnvelope {
    crate::v3_envelope::build_response_envelope(
        request_id,
        v3::server_envelope::Payload::RuntimeSnapshot(snapshot),
    )
}

/// Build an `OutputDelta` push event.
#[must_use]
pub fn build_output_delta(
    runtime_id: uuid::Uuid,
    pane_id: uuid::Uuid,
    data: bytes::Bytes,
    pane_output_seq: u64,
) -> v3::OutputDelta {
    v3::OutputDelta {
        runtime_id: crate::uuid_to_bytes(runtime_id),
        pane_id: crate::uuid_to_bytes(pane_id),
        data,
        pane_output_seq,
    }
}

/// Build a `ServerEnvelope` push event for an `OutputDelta`.
#[must_use]
pub fn build_output_delta_envelope(
    runtime_id: uuid::Uuid,
    pane_id: uuid::Uuid,
    data: bytes::Bytes,
    pane_output_seq: u64,
) -> v3::ServerEnvelope {
    crate::v3_envelope::build_push_envelope(v3::server_envelope::Payload::OutputDelta(
        build_output_delta(runtime_id, pane_id, data, pane_output_seq),
    ))
}

/// Detect a gap in `pane_output_seq` between the expected next value and
/// the received value.
///
/// Returns `Some(gap)` if `received > expected` (messages were dropped),
/// `None` if the sequence is contiguous (`received == expected`).
///
/// A `received < expected` value indicates a protocol violation or resync
/// and is also returned as `None` — the caller should handle that via
/// the `StreamOverflow` event path, not sequence gap detection.
#[must_use]
pub fn detect_output_seq_gap(expected_next: u64, received: u64) -> Option<u64> {
    if received > expected_next { Some(received - expected_next) } else { None }
}

/// Truncate scrollback data to at most `limit` bytes from the tail.
///
/// Returns the truncated tail and whether the result is complete
/// (i.e., no truncation occurred).
#[must_use]
pub fn truncate_scrollback(data: &[u8], limit: usize) -> (bytes::Bytes, bool) {
    if data.len() <= limit {
        (bytes::Bytes::copy_from_slice(data), true)
    } else {
        let start = data.len() - limit;
        (bytes::Bytes::copy_from_slice(&data[start..]), false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode_frame, encode_frame, uuid_to_bytes};
    use bytes::BytesMut;

    fn rt() -> uuid::Uuid {
        uuid::Uuid::new_v4()
    }

    fn pn() -> uuid::Uuid {
        uuid::Uuid::new_v4()
    }

    fn default_modes() -> v3::TerminalModeState {
        v3::TerminalModeState::default()
    }

    // ── build_pane_snapshot ──

    #[test]
    fn pane_snapshot_populates_all_fields() {
        let p = pn();
        let modes = v3::TerminalModeState { bracketed_paste: true, ..Default::default() };
        let snap = build_pane_snapshot(PaneSnapshotParams {
            pane_id: p,
            pane_output_seq: 42,
            title: "bash".into(),
            cwd: "/home/user".into(),
            cols: 120,
            rows: 40,
            exit_status: None,
            terminal_modes: modes,
            scrollback_tail: bytes::Bytes::from_static(b"$ ls\n"),
            total_scrollback_bytes: 5,
        });
        assert_eq!(snap.pane_id, uuid_to_bytes(p));
        assert_eq!(snap.pane_output_seq, 42);
        assert_eq!(snap.title, "bash");
        assert_eq!(snap.cwd, "/home/user");
        assert_eq!(snap.cols, 120);
        assert_eq!(snap.rows, 40);
        assert_eq!(snap.exit_status, None);
        assert!(snap.terminal_modes.unwrap().bracketed_paste);
        assert_eq!(snap.scrollback_tail.as_ref(), b"$ ls\n");
        assert_eq!(snap.total_scrollback_bytes, 5);
        assert!(snap.scrollback_complete);
    }

    #[test]
    fn pane_snapshot_with_exit_status() {
        let snap = build_pane_snapshot(PaneSnapshotParams {
            pane_id: pn(),
            pane_output_seq: 0,
            title: "zsh".into(),
            cwd: "/tmp".into(),
            cols: 80,
            rows: 24,
            exit_status: Some(1),
            terminal_modes: default_modes(),
            scrollback_tail: bytes::Bytes::new(),
            total_scrollback_bytes: 0,
        });
        assert_eq!(snap.exit_status, Some(1));
    }

    #[test]
    fn pane_snapshot_scrollback_complete_when_tail_equals_total() {
        let snap = build_pane_snapshot(PaneSnapshotParams {
            pane_id: pn(),
            pane_output_seq: 0,
            title: String::new(),
            cwd: String::new(),
            cols: 80,
            rows: 24,
            exit_status: None,
            terminal_modes: default_modes(),
            scrollback_tail: bytes::Bytes::from_static(b"hello"),
            total_scrollback_bytes: 5,
        });
        assert!(snap.scrollback_complete);
    }

    #[test]
    fn pane_snapshot_scrollback_complete_when_tail_exceeds_total() {
        let snap = build_pane_snapshot(PaneSnapshotParams {
            pane_id: pn(),
            pane_output_seq: 0,
            title: String::new(),
            cwd: String::new(),
            cols: 80,
            rows: 24,
            exit_status: None,
            terminal_modes: default_modes(),
            scrollback_tail: bytes::Bytes::from_static(b"hello world"),
            total_scrollback_bytes: 5,
        });
        assert!(snap.scrollback_complete);
    }

    #[test]
    fn pane_snapshot_scrollback_incomplete_when_tail_less_than_total() {
        let snap = build_pane_snapshot(PaneSnapshotParams {
            pane_id: pn(),
            pane_output_seq: 0,
            title: String::new(),
            cwd: String::new(),
            cols: 80,
            rows: 24,
            exit_status: None,
            terminal_modes: default_modes(),
            scrollback_tail: bytes::Bytes::from_static(b"tail"),
            total_scrollback_bytes: 1000,
        });
        assert!(!snap.scrollback_complete);
    }

    #[test]
    fn pane_snapshot_empty_scrollback_is_complete_when_total_zero() {
        let snap = build_pane_snapshot(PaneSnapshotParams {
            pane_id: pn(),
            pane_output_seq: 0,
            title: String::new(),
            cwd: String::new(),
            cols: 80,
            rows: 24,
            exit_status: None,
            terminal_modes: default_modes(),
            scrollback_tail: bytes::Bytes::new(),
            total_scrollback_bytes: 0,
        });
        assert!(snap.scrollback_complete);
    }

    #[test]
    fn pane_snapshot_wire_roundtrip() {
        let snap = build_pane_snapshot(PaneSnapshotParams {
            pane_id: pn(),
            pane_output_seq: 100,
            title: "vim".into(),
            cwd: "/project".into(),
            cols: 132,
            rows: 43,
            exit_status: Some(0),
            terminal_modes: v3::TerminalModeState {
                alternate_screen: true,
                cursor_hidden: true,
                ..Default::default()
            },
            scrollback_tail: bytes::Bytes::from_static(b"scrollback data here"),
            total_scrollback_bytes: 4096,
        });
        let mut buf = BytesMut::new();
        encode_frame(&snap, &mut buf).unwrap();
        let decoded: v3::PaneSnapshot = decode_frame(&mut buf).unwrap();
        assert_eq!(snap, decoded);
    }

    // ── build_runtime_snapshot ──

    #[test]
    fn runtime_snapshot_populates_all_fields() {
        let r = rt();
        let p = pn();
        let pane = build_pane_snapshot(PaneSnapshotParams {
            pane_id: p,
            pane_output_seq: 1,
            title: "bash".into(),
            cwd: "/home".into(),
            cols: 80,
            rows: 24,
            exit_status: None,
            terminal_modes: default_modes(),
            scrollback_tail: bytes::Bytes::from_static(b"$ "),
            total_scrollback_bytes: 2,
        });
        let snap = build_runtime_snapshot(r, 42, v3::RuntimeClientRole::Writer, vec![pane]);
        assert_eq!(snap.runtime_id, uuid_to_bytes(r));
        assert_eq!(snap.runtime_revision, 42);
        assert_eq!(snap.client_role, v3::RuntimeClientRole::Writer as i32);
        assert_eq!(snap.panes.len(), 1);
        assert_eq!(snap.panes[0].pane_id, uuid_to_bytes(p));
    }

    #[test]
    fn runtime_snapshot_empty_panes() {
        let snap = build_runtime_snapshot(rt(), 1, v3::RuntimeClientRole::Reader, vec![]);
        assert!(snap.panes.is_empty());
        assert_eq!(snap.client_role, v3::RuntimeClientRole::Reader as i32);
    }

    #[test]
    fn runtime_snapshot_multiple_panes() {
        let panes: Vec<v3::PaneSnapshot> = (0..3)
            .map(|i| {
                build_pane_snapshot(PaneSnapshotParams {
                    pane_id: pn(),
                    pane_output_seq: i,
                    title: format!("pane-{i}"),
                    cwd: "/tmp".into(),
                    cols: 80,
                    rows: 24,
                    exit_status: None,
                    terminal_modes: default_modes(),
                    scrollback_tail: bytes::Bytes::new(),
                    total_scrollback_bytes: 0,
                })
            })
            .collect();
        let snap = build_runtime_snapshot(rt(), 10, v3::RuntimeClientRole::Writer, panes);
        assert_eq!(snap.panes.len(), 3);
    }

    #[test]
    fn runtime_snapshot_wire_roundtrip() {
        let pane = build_pane_snapshot(PaneSnapshotParams {
            pane_id: pn(),
            pane_output_seq: 50,
            title: "bash".into(),
            cwd: "/home/user".into(),
            cols: 120,
            rows: 40,
            exit_status: None,
            terminal_modes: v3::TerminalModeState { bracketed_paste: true, ..Default::default() },
            scrollback_tail: bytes::Bytes::from_static(b"$ ls\nfile.txt\n"),
            total_scrollback_bytes: 4096,
        });
        let snap = build_runtime_snapshot(rt(), 42, v3::RuntimeClientRole::Writer, vec![pane]);
        let mut buf = BytesMut::new();
        encode_frame(&snap, &mut buf).unwrap();
        let decoded: v3::RuntimeSnapshot = decode_frame(&mut buf).unwrap();
        assert_eq!(snap, decoded);
    }

    // ── build_snapshot_response ──

    #[test]
    fn snapshot_response_echoes_request_id() {
        let snap = build_runtime_snapshot(rt(), 1, v3::RuntimeClientRole::Writer, vec![]);
        let env = build_snapshot_response(7, snap.clone());
        assert_eq!(env.request_id, 7);
        match env.payload {
            Some(v3::server_envelope::Payload::RuntimeSnapshot(ref s)) => {
                assert_eq!(s, &snap);
            }
            _ => panic!("expected RuntimeSnapshot payload"),
        }
    }

    #[test]
    fn snapshot_response_is_not_push_event() {
        let snap = build_runtime_snapshot(rt(), 1, v3::RuntimeClientRole::Writer, vec![]);
        let env = build_snapshot_response(42, snap);
        assert!(!crate::v3_envelope::is_push_event(&env));
    }

    #[test]
    fn snapshot_response_wire_roundtrip() {
        let pane = build_pane_snapshot(PaneSnapshotParams {
            pane_id: pn(),
            pane_output_seq: 10,
            title: "zsh".into(),
            cwd: "/".into(),
            cols: 80,
            rows: 24,
            exit_status: None,
            terminal_modes: default_modes(),
            scrollback_tail: bytes::Bytes::from_static(b"data"),
            total_scrollback_bytes: 100,
        });
        let snap = build_runtime_snapshot(rt(), 5, v3::RuntimeClientRole::Reader, vec![pane]);
        let env = build_snapshot_response(99, snap);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    // ── build_output_delta ──

    #[test]
    fn output_delta_populates_all_fields() {
        let r = rt();
        let p = pn();
        let delta = build_output_delta(r, p, bytes::Bytes::from_static(b"output"), 42);
        assert_eq!(delta.runtime_id, uuid_to_bytes(r));
        assert_eq!(delta.pane_id, uuid_to_bytes(p));
        assert_eq!(delta.data.as_ref(), b"output");
        assert_eq!(delta.pane_output_seq, 42);
    }

    #[test]
    fn output_delta_wire_roundtrip() {
        let delta =
            build_output_delta(rt(), pn(), bytes::Bytes::from_static(b"terminal output"), 100);
        let mut buf = BytesMut::new();
        encode_frame(&delta, &mut buf).unwrap();
        let decoded: v3::OutputDelta = decode_frame(&mut buf).unwrap();
        assert_eq!(delta, decoded);
    }

    // ── build_output_delta_envelope ──

    #[test]
    fn output_delta_envelope_is_push_event() {
        let env = build_output_delta_envelope(rt(), pn(), bytes::Bytes::from_static(b"x"), 1);
        assert_eq!(env.request_id, 0);
        assert!(crate::v3_envelope::is_push_event(&env));
    }

    #[test]
    fn output_delta_envelope_contains_correct_payload() {
        let r = rt();
        let p = pn();
        let env = build_output_delta_envelope(r, p, bytes::Bytes::from_static(b"data"), 5);
        match env.payload {
            Some(v3::server_envelope::Payload::OutputDelta(ref d)) => {
                assert_eq!(d.runtime_id, uuid_to_bytes(r));
                assert_eq!(d.pane_id, uuid_to_bytes(p));
                assert_eq!(d.data.as_ref(), b"data");
                assert_eq!(d.pane_output_seq, 5);
            }
            _ => panic!("expected OutputDelta payload"),
        }
    }

    #[test]
    fn output_delta_envelope_wire_roundtrip() {
        let env =
            build_output_delta_envelope(rt(), pn(), bytes::Bytes::from_static(b"output bytes"), 99);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    // ── detect_output_seq_gap ──

    #[test]
    fn no_gap_when_contiguous() {
        assert_eq!(detect_output_seq_gap(5, 5), None);
    }

    #[test]
    fn gap_detected_when_received_ahead() {
        assert_eq!(detect_output_seq_gap(5, 8), Some(3));
    }

    #[test]
    fn single_message_gap() {
        // Expected 5, got 6 — one message (seq 5) was dropped.
        assert_eq!(detect_output_seq_gap(5, 6), Some(1));
    }

    #[test]
    fn no_gap_when_received_behind_expected() {
        assert_eq!(detect_output_seq_gap(10, 5), None);
    }

    #[test]
    fn no_gap_at_zero() {
        assert_eq!(detect_output_seq_gap(0, 0), None);
    }

    #[test]
    fn gap_from_zero() {
        assert_eq!(detect_output_seq_gap(0, 3), Some(3));
    }

    // ── truncate_scrollback ──

    #[test]
    fn truncate_no_op_when_within_limit() {
        let data = b"hello world";
        let (tail, complete) = truncate_scrollback(data, 100);
        assert_eq!(tail.as_ref(), data);
        assert!(complete);
    }

    #[test]
    fn truncate_exact_limit() {
        let data = b"12345";
        let (tail, complete) = truncate_scrollback(data, 5);
        assert_eq!(tail.as_ref(), data);
        assert!(complete);
    }

    #[test]
    fn truncate_takes_tail_bytes() {
        let data = b"abcdefghij";
        let (tail, complete) = truncate_scrollback(data, 5);
        assert_eq!(tail.as_ref(), b"fghij");
        assert!(!complete);
    }

    #[test]
    fn truncate_empty_data() {
        let (tail, complete) = truncate_scrollback(b"", 100);
        assert!(tail.is_empty());
        assert!(complete);
    }

    #[test]
    fn truncate_zero_limit() {
        let (tail, complete) = truncate_scrollback(b"data", 0);
        assert!(tail.is_empty());
        assert!(!complete);
    }

    #[test]
    fn truncate_single_byte_limit() {
        let data = b"abcde";
        let (tail, complete) = truncate_scrollback(data, 1);
        assert_eq!(tail.as_ref(), b"e");
        assert!(!complete);
    }

    // ── DEFAULT_SCROLLBACK_TAIL_LIMIT ──

    #[test]
    fn default_scrollback_limit_is_256kb() {
        assert_eq!(DEFAULT_SCROLLBACK_TAIL_LIMIT, 256 * 1024);
    }

    // ── Integration: snapshot with truncated scrollback ──

    #[test]
    fn snapshot_with_truncated_scrollback() {
        let full_scrollback = vec![b'x'; 1_000_000];
        let (tail, complete) = truncate_scrollback(&full_scrollback, DEFAULT_SCROLLBACK_TAIL_LIMIT);
        assert!(!complete);
        assert_eq!(tail.len(), DEFAULT_SCROLLBACK_TAIL_LIMIT);

        let snap = build_pane_snapshot(PaneSnapshotParams {
            pane_id: pn(),
            pane_output_seq: 0,
            title: "bash".into(),
            cwd: "/home".into(),
            cols: 80,
            rows: 24,
            exit_status: None,
            terminal_modes: default_modes(),
            scrollback_tail: tail,
            total_scrollback_bytes: full_scrollback.len() as u64,
        });
        assert!(!snap.scrollback_complete);
        assert_eq!(snap.total_scrollback_bytes, 1_000_000);
        assert_eq!(snap.scrollback_tail.len(), DEFAULT_SCROLLBACK_TAIL_LIMIT);
    }

    // ── Integration: output sequence continuity tracking ──

    #[test]
    fn output_seq_continuity_across_deltas() {
        let r = rt();
        let p = pn();
        let mut expected_seq = 1_u64;

        for i in 1..=5 {
            let delta = build_output_delta(r, p, bytes::Bytes::from(format!("output-{i}")), i);
            assert_eq!(detect_output_seq_gap(expected_seq, delta.pane_output_seq), None);
            expected_seq = delta.pane_output_seq + 1;
        }
    }

    #[test]
    fn output_seq_gap_triggers_resync_path() {
        let mut expected_seq = 1_u64;

        // Receive seq 1, 2, then skip to 5
        for seq in [1, 2, 5] {
            let gap = detect_output_seq_gap(expected_seq, seq);
            if seq == 5 {
                assert_eq!(gap, Some(2));
            } else {
                assert_eq!(gap, None);
                expected_seq = seq + 1;
            }
        }
    }
}
