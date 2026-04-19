//! V3 terminal input: builders and daemon-side resolution.
//!
//! Implements RFC-021 Section 4 (Terminal Input Model).
//!
//! The client sends structured input variants (`RawInput`, `PasteInput`,
//! `FocusInput`) via `TerminalInput`. The daemon resolves each variant
//! to raw bytes using the pane's current `TerminalModeState`:
//!
//! - `RawInput` passes through unchanged.
//! - `PasteInput` wraps with bracketed paste sequences when the mode is active.
//! - `FocusInput` generates focus in/out escape sequences when focus reporting
//!   is active.

use crate::v3;

/// Bracketed paste mode start sequence (DECSET 2004).
const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";

/// Bracketed paste mode end sequence (DECSET 2004).
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

/// Focus-in escape sequence (DECSET 1004).
const FOCUS_IN: &[u8] = b"\x1b[I";

/// Focus-out escape sequence (DECSET 1004).
const FOCUS_OUT: &[u8] = b"\x1b[O";

/// Build a `TerminalInput` with `RawInput` kind.
#[must_use]
pub fn build_raw_input(
    runtime_id: uuid::Uuid,
    pane_id: uuid::Uuid,
    data: bytes::Bytes,
) -> v3::TerminalInput {
    v3::TerminalInput {
        runtime_id: crate::uuid_to_bytes(runtime_id),
        pane_id: crate::uuid_to_bytes(pane_id),
        kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput { data })),
    }
}

/// Build a `TerminalInput` with `PasteInput` kind.
#[must_use]
pub fn build_paste_input(
    runtime_id: uuid::Uuid,
    pane_id: uuid::Uuid,
    text: bytes::Bytes,
) -> v3::TerminalInput {
    v3::TerminalInput {
        runtime_id: crate::uuid_to_bytes(runtime_id),
        pane_id: crate::uuid_to_bytes(pane_id),
        kind: Some(v3::terminal_input::Kind::Paste(v3::PasteInput { text })),
    }
}

/// Build a `TerminalInput` with `FocusInput` kind.
#[must_use]
pub fn build_focus_input(
    runtime_id: uuid::Uuid,
    pane_id: uuid::Uuid,
    focused: bool,
) -> v3::TerminalInput {
    v3::TerminalInput {
        runtime_id: crate::uuid_to_bytes(runtime_id),
        pane_id: crate::uuid_to_bytes(pane_id),
        kind: Some(v3::terminal_input::Kind::Focus(v3::FocusInput { focused })),
    }
}

/// Build a `ClientEnvelope` wrapping a `TerminalInput`.
#[must_use]
pub fn build_terminal_input_envelope(input: v3::TerminalInput) -> v3::ClientEnvelope {
    // TerminalInput is fire-and-forget: request_id = 0.
    v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::TerminalInput(input)),
    }
}

/// Resolve a `TerminalInput` kind to raw bytes for PTY write.
///
/// Uses the pane's current terminal mode state to decide wrapping:
/// - `RawInput`: returned as-is.
/// - `PasteInput`: wrapped with bracketed paste sequences when
///   `modes.bracketed_paste` is true; sent as plain text otherwise.
/// - `FocusInput`: generates focus in/out escape sequence when
///   `modes.focus_reporting` is true; returns empty when inactive.
/// - Missing kind: returns empty.
#[must_use]
pub fn resolve_input(
    kind: Option<&v3::terminal_input::Kind>,
    modes: &v3::TerminalModeState,
) -> Vec<u8> {
    match kind {
        Some(v3::terminal_input::Kind::Raw(raw)) => raw.data.to_vec(),
        Some(v3::terminal_input::Kind::Paste(paste)) => {
            if modes.bracketed_paste {
                let mut buf = Vec::with_capacity(
                    BRACKETED_PASTE_START.len() + paste.text.len() + BRACKETED_PASTE_END.len(),
                );
                buf.extend_from_slice(BRACKETED_PASTE_START);
                buf.extend_from_slice(&paste.text);
                buf.extend_from_slice(BRACKETED_PASTE_END);
                buf
            } else {
                paste.text.to_vec()
            }
        }
        Some(v3::terminal_input::Kind::Focus(focus)) => {
            if modes.focus_reporting {
                if focus.focused { FOCUS_IN.to_vec() } else { FOCUS_OUT.to_vec() }
            } else {
                Vec::new()
            }
        }
        None => Vec::new(),
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

    fn modes_default() -> v3::TerminalModeState {
        v3::TerminalModeState::default()
    }

    fn modes_with_bracketed_paste() -> v3::TerminalModeState {
        v3::TerminalModeState { bracketed_paste: true, ..Default::default() }
    }

    fn modes_with_focus_reporting() -> v3::TerminalModeState {
        v3::TerminalModeState { focus_reporting: true, ..Default::default() }
    }

    fn modes_all_active() -> v3::TerminalModeState {
        v3::TerminalModeState { bracketed_paste: true, focus_reporting: true, ..Default::default() }
    }

    // ── build_raw_input ──

    #[test]
    fn raw_input_populates_all_fields() {
        let r = rt();
        let p = pn();
        let input = build_raw_input(r, p, bytes::Bytes::from_static(b"hello"));
        assert_eq!(input.runtime_id, uuid_to_bytes(r));
        assert_eq!(input.pane_id, uuid_to_bytes(p));
        match input.kind {
            Some(v3::terminal_input::Kind::Raw(raw)) => {
                assert_eq!(raw.data.as_ref(), b"hello");
            }
            _ => panic!("expected RawInput kind"),
        }
    }

    #[test]
    fn raw_input_wire_roundtrip() {
        let input = build_raw_input(rt(), pn(), bytes::Bytes::from_static(b"test"));
        let mut buf = BytesMut::new();
        encode_frame(&input, &mut buf).unwrap();
        let decoded: v3::TerminalInput = decode_frame(&mut buf).unwrap();
        assert_eq!(input, decoded);
    }

    // ── build_paste_input ──

    #[test]
    fn paste_input_populates_all_fields() {
        let r = rt();
        let p = pn();
        let input = build_paste_input(r, p, bytes::Bytes::from_static(b"pasted"));
        assert_eq!(input.runtime_id, uuid_to_bytes(r));
        assert_eq!(input.pane_id, uuid_to_bytes(p));
        match input.kind {
            Some(v3::terminal_input::Kind::Paste(paste)) => {
                assert_eq!(paste.text.as_ref(), b"pasted");
            }
            _ => panic!("expected PasteInput kind"),
        }
    }

    #[test]
    fn paste_input_wire_roundtrip() {
        let input = build_paste_input(rt(), pn(), bytes::Bytes::from_static(b"clipboard"));
        let mut buf = BytesMut::new();
        encode_frame(&input, &mut buf).unwrap();
        let decoded: v3::TerminalInput = decode_frame(&mut buf).unwrap();
        assert_eq!(input, decoded);
    }

    // ── build_focus_input ──

    #[test]
    fn focus_input_focused_true() {
        let r = rt();
        let p = pn();
        let input = build_focus_input(r, p, true);
        assert_eq!(input.runtime_id, uuid_to_bytes(r));
        assert_eq!(input.pane_id, uuid_to_bytes(p));
        match input.kind {
            Some(v3::terminal_input::Kind::Focus(focus)) => assert!(focus.focused),
            _ => panic!("expected FocusInput kind"),
        }
    }

    #[test]
    fn focus_input_focused_false() {
        let input = build_focus_input(rt(), pn(), false);
        match input.kind {
            Some(v3::terminal_input::Kind::Focus(focus)) => assert!(!focus.focused),
            _ => panic!("expected FocusInput kind"),
        }
    }

    #[test]
    fn focus_input_wire_roundtrip() {
        for focused in [true, false] {
            let input = build_focus_input(rt(), pn(), focused);
            let mut buf = BytesMut::new();
            encode_frame(&input, &mut buf).unwrap();
            let decoded: v3::TerminalInput = decode_frame(&mut buf).unwrap();
            assert_eq!(input, decoded);
        }
    }

    // ── build_terminal_input_envelope ──

    #[test]
    fn envelope_is_fire_and_forget() {
        let input = build_raw_input(rt(), pn(), bytes::Bytes::from_static(b"x"));
        let env = build_terminal_input_envelope(input);
        assert_eq!(env.request_id, 0);
    }

    #[test]
    fn envelope_wire_roundtrip() {
        let input = build_paste_input(rt(), pn(), bytes::Bytes::from_static(b"text"));
        let env = build_terminal_input_envelope(input);
        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ClientEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    // ── resolve_input: RawInput ──

    #[test]
    fn resolve_raw_passes_through() {
        let kind = v3::terminal_input::Kind::Raw(v3::RawInput {
            data: bytes::Bytes::from_static(b"hello"),
        });
        let result = resolve_input(Some(&kind), &modes_default());
        assert_eq!(result, b"hello");
    }

    #[test]
    fn resolve_raw_ignores_modes() {
        let kind = v3::terminal_input::Kind::Raw(v3::RawInput {
            data: bytes::Bytes::from_static(b"data"),
        });
        let result = resolve_input(Some(&kind), &modes_all_active());
        assert_eq!(result, b"data");
    }

    #[test]
    fn resolve_raw_empty_data() {
        let kind = v3::terminal_input::Kind::Raw(v3::RawInput { data: bytes::Bytes::new() });
        let result = resolve_input(Some(&kind), &modes_default());
        assert!(result.is_empty());
    }

    // ── resolve_input: PasteInput with bracketed paste active ──

    #[test]
    fn resolve_paste_wraps_when_bracketed_paste_active() {
        let kind = v3::terminal_input::Kind::Paste(v3::PasteInput {
            text: bytes::Bytes::from_static(b"pasted text"),
        });
        let result = resolve_input(Some(&kind), &modes_with_bracketed_paste());
        let mut expected = Vec::new();
        expected.extend_from_slice(b"\x1b[200~");
        expected.extend_from_slice(b"pasted text");
        expected.extend_from_slice(b"\x1b[201~");
        assert_eq!(result, expected);
    }

    #[test]
    fn resolve_paste_no_wrap_when_bracketed_paste_inactive() {
        let kind = v3::terminal_input::Kind::Paste(v3::PasteInput {
            text: bytes::Bytes::from_static(b"pasted text"),
        });
        let result = resolve_input(Some(&kind), &modes_default());
        assert_eq!(result, b"pasted text");
    }

    #[test]
    fn resolve_paste_empty_text_with_bracketed_paste() {
        let kind = v3::terminal_input::Kind::Paste(v3::PasteInput { text: bytes::Bytes::new() });
        let result = resolve_input(Some(&kind), &modes_with_bracketed_paste());
        let mut expected = Vec::new();
        expected.extend_from_slice(b"\x1b[200~");
        expected.extend_from_slice(b"\x1b[201~");
        assert_eq!(result, expected);
    }

    #[test]
    fn resolve_paste_empty_text_without_bracketed_paste() {
        let kind = v3::terminal_input::Kind::Paste(v3::PasteInput { text: bytes::Bytes::new() });
        let result = resolve_input(Some(&kind), &modes_default());
        assert!(result.is_empty());
    }

    #[test]
    fn resolve_paste_non_utf8_bytes() {
        let kind = v3::terminal_input::Kind::Paste(v3::PasteInput {
            text: bytes::Bytes::from_static(&[0xFF, 0xFE, 0x00, 0x01]),
        });
        let result = resolve_input(Some(&kind), &modes_with_bracketed_paste());
        assert!(result.starts_with(b"\x1b[200~"));
        assert!(result.ends_with(b"\x1b[201~"));
        assert_eq!(result.len(), 6 + 4 + 6);
    }

    // ── resolve_input: FocusInput with focus reporting active ──

    #[test]
    fn resolve_focus_in_when_reporting_active() {
        let kind = v3::terminal_input::Kind::Focus(v3::FocusInput { focused: true });
        let result = resolve_input(Some(&kind), &modes_with_focus_reporting());
        assert_eq!(result, b"\x1b[I");
    }

    #[test]
    fn resolve_focus_out_when_reporting_active() {
        let kind = v3::terminal_input::Kind::Focus(v3::FocusInput { focused: false });
        let result = resolve_input(Some(&kind), &modes_with_focus_reporting());
        assert_eq!(result, b"\x1b[O");
    }

    #[test]
    fn resolve_focus_empty_when_reporting_inactive() {
        for focused in [true, false] {
            let kind = v3::terminal_input::Kind::Focus(v3::FocusInput { focused });
            let result = resolve_input(Some(&kind), &modes_default());
            assert!(result.is_empty());
        }
    }

    // ── resolve_input: None kind ──

    #[test]
    fn resolve_none_kind_returns_empty() {
        let result = resolve_input(None, &modes_default());
        assert!(result.is_empty());
    }

    #[test]
    fn resolve_none_kind_ignores_modes() {
        let result = resolve_input(None, &modes_all_active());
        assert!(result.is_empty());
    }

    // ── resolve_input: mode combinations ──

    #[test]
    fn resolve_paste_with_all_modes_only_uses_bracketed_paste() {
        let kind = v3::terminal_input::Kind::Paste(v3::PasteInput {
            text: bytes::Bytes::from_static(b"text"),
        });
        let result = resolve_input(Some(&kind), &modes_all_active());
        assert!(result.starts_with(b"\x1b[200~"));
        assert!(result.ends_with(b"\x1b[201~"));
        assert_eq!(result.len(), 6 + 4 + 6);
    }

    #[test]
    fn resolve_focus_with_all_modes_generates_sequence() {
        let kind = v3::terminal_input::Kind::Focus(v3::FocusInput { focused: true });
        let result = resolve_input(Some(&kind), &modes_all_active());
        assert_eq!(result, b"\x1b[I");
    }
}
