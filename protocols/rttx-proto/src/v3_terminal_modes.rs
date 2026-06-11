//! V3 terminal mode state: builders and conversions.
//!
//! Implements RFC-021 Section 5 (Terminal Interaction State).
//!
//! Provides conversion between the daemon's per-field mode tracking
//! (individual booleans and a `u16` mouse tracking value) and the
//! consolidated `TerminalModeState` protobuf message. Also provides
//! builders for `TerminalModeChanged` push events.

use crate::v3;

/// Convert a raw mouse tracking mode value (0, 1000, 1002, 1003) to the
/// v3 `MouseMode` enum.
///
/// Unknown values map to `MouseMode::None`.
#[must_use]
pub fn mouse_mode_from_tracking_value(value: u16) -> v3::MouseMode {
    match value {
        1000 => v3::MouseMode::Normal,
        1002 => v3::MouseMode::Button,
        1003 => v3::MouseMode::Any,
        _ => v3::MouseMode::None,
    }
}

/// Convert a v3 `MouseMode` enum back to the raw tracking value used by
/// the daemon's `PaneScreen`.
#[must_use]
pub fn tracking_value_from_mouse_mode(mode: v3::MouseMode) -> u16 {
    match mode {
        v3::MouseMode::None => 0,
        v3::MouseMode::X10 => 9,
        v3::MouseMode::Normal => 1000,
        v3::MouseMode::Button => 1002,
        v3::MouseMode::Any => 1003,
    }
}

/// Build a `TerminalModeChanged` push event.
#[must_use]
pub fn build_terminal_mode_changed(
    runtime_id: uuid::Uuid,
    pane_id: uuid::Uuid,
    workspace_revision: u64,
    modes: v3::TerminalModeState,
) -> v3::TerminalModeChanged {
    v3::TerminalModeChanged {
        runtime_id: crate::uuid_to_bytes(runtime_id),
        pane_id: crate::uuid_to_bytes(pane_id),
        workspace_revision,
        modes: Some(modes),
    }
}

/// Build a `ServerEnvelope` push event for a terminal mode change.
#[must_use]
pub fn build_mode_changed_envelope(
    runtime_id: uuid::Uuid,
    pane_id: uuid::Uuid,
    workspace_revision: u64,
    modes: v3::TerminalModeState,
) -> v3::ServerEnvelope {
    crate::v3_envelope::build_push_envelope(v3::server_envelope::Payload::TerminalModeChanged(
        build_terminal_mode_changed(runtime_id, pane_id, workspace_revision, modes),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode_frame, encode_frame, uuid_to_bytes};
    use bytes::BytesMut;

    fn mode_state(
        bracketed_paste: bool,
        mouse_tracking: u16,
        sgr_mouse: bool,
    ) -> v3::TerminalModeState {
        v3::TerminalModeState {
            bracketed_paste,
            mouse_mode: mouse_mode_from_tracking_value(mouse_tracking) as i32,
            sgr_mouse,
            ..Default::default()
        }
    }

    // ── MouseMode conversion ──

    #[test]
    fn mouse_mode_from_zero_is_none() {
        assert_eq!(mouse_mode_from_tracking_value(0), v3::MouseMode::None);
    }

    #[test]
    fn mouse_mode_from_1000_is_normal() {
        assert_eq!(mouse_mode_from_tracking_value(1000), v3::MouseMode::Normal);
    }

    #[test]
    fn mouse_mode_from_1002_is_button() {
        assert_eq!(mouse_mode_from_tracking_value(1002), v3::MouseMode::Button);
    }

    #[test]
    fn mouse_mode_from_1003_is_any() {
        assert_eq!(mouse_mode_from_tracking_value(1003), v3::MouseMode::Any);
    }

    #[test]
    fn mouse_mode_unknown_value_maps_to_none() {
        assert_eq!(mouse_mode_from_tracking_value(9999), v3::MouseMode::None);
    }

    #[test]
    fn tracking_value_roundtrip_none() {
        let mode = mouse_mode_from_tracking_value(0);
        assert_eq!(tracking_value_from_mouse_mode(mode), 0);
    }

    #[test]
    fn tracking_value_roundtrip_normal() {
        let mode = mouse_mode_from_tracking_value(1000);
        assert_eq!(tracking_value_from_mouse_mode(mode), 1000);
    }

    #[test]
    fn tracking_value_roundtrip_button() {
        let mode = mouse_mode_from_tracking_value(1002);
        assert_eq!(tracking_value_from_mouse_mode(mode), 1002);
    }

    #[test]
    fn tracking_value_roundtrip_any() {
        let mode = mouse_mode_from_tracking_value(1003);
        assert_eq!(tracking_value_from_mouse_mode(mode), 1003);
    }

    #[test]
    fn tracking_value_x10() {
        assert_eq!(tracking_value_from_mouse_mode(v3::MouseMode::X10), 9);
    }

    // ── TerminalModeState construction ──

    #[test]
    fn default_mode_state_all_false() {
        let state = v3::TerminalModeState::default();
        assert!(!state.bracketed_paste);
        assert!(!state.focus_reporting);
        assert!(!state.application_cursor_keys);
        assert!(!state.application_keypad);
        assert!(!state.alternate_screen);
        assert!(!state.cursor_hidden);
        assert_eq!(state.mouse_mode, v3::MouseMode::None as i32);
        assert!(!state.sgr_mouse);
    }

    #[test]
    fn mode_state_with_all_active() {
        let state = v3::TerminalModeState {
            bracketed_paste: true,
            focus_reporting: true,
            application_cursor_keys: true,
            application_keypad: true,
            alternate_screen: true,
            cursor_hidden: true,
            mouse_mode: v3::MouseMode::Any as i32,
            sgr_mouse: true,
        };
        assert!(state.bracketed_paste);
        assert!(state.focus_reporting);
        assert!(state.application_cursor_keys);
        assert!(state.application_keypad);
        assert!(state.alternate_screen);
        assert!(state.cursor_hidden);
        assert_eq!(state.mouse_mode, v3::MouseMode::Any as i32);
        assert!(state.sgr_mouse);
    }

    #[test]
    fn mode_state_mouse_mode_conversion() {
        let state = mode_state(false, 1002, false);
        assert_eq!(state.mouse_mode, v3::MouseMode::Button as i32);
    }

    // ── build_terminal_mode_changed ──

    #[test]
    fn build_mode_changed_populates_all_fields() {
        let rt = uuid::Uuid::new_v4();
        let pn = uuid::Uuid::new_v4();
        let modes = mode_state(true, 0, false);
        let msg = build_terminal_mode_changed(rt, pn, 42, modes);
        assert_eq!(msg.runtime_id, uuid_to_bytes(rt));
        assert_eq!(msg.pane_id, uuid_to_bytes(pn));
        assert_eq!(msg.workspace_revision, 42);
        assert_eq!(msg.modes, Some(modes));
    }

    #[test]
    fn mode_changed_wire_roundtrip() {
        let rt = uuid::Uuid::new_v4();
        let pn = uuid::Uuid::new_v4();
        let modes = v3::TerminalModeState {
            bracketed_paste: true,
            focus_reporting: true,
            application_cursor_keys: true,
            application_keypad: true,
            alternate_screen: true,
            cursor_hidden: true,
            mouse_mode: v3::MouseMode::Any as i32,
            sgr_mouse: true,
        };
        let msg = build_terminal_mode_changed(rt, pn, 99, modes);

        let mut buf = BytesMut::new();
        encode_frame(&msg, &mut buf).unwrap();
        let decoded: v3::TerminalModeChanged = decode_frame(&mut buf).unwrap();
        assert_eq!(msg, decoded);
    }

    // ── build_mode_changed_envelope ──

    #[test]
    fn mode_changed_envelope_is_push_event() {
        let rt = uuid::Uuid::new_v4();
        let pn = uuid::Uuid::new_v4();
        let modes = mode_state(true, 0, false);
        let env = build_mode_changed_envelope(rt, pn, 5, modes);
        assert_eq!(env.request_id, 0);
        assert!(crate::v3_envelope::is_push_event(&env));
    }

    #[test]
    fn mode_changed_envelope_wire_roundtrip() {
        let rt = uuid::Uuid::new_v4();
        let pn = uuid::Uuid::new_v4();
        let modes = v3::TerminalModeState {
            focus_reporting: true,
            application_keypad: true,
            cursor_hidden: true,
            mouse_mode: v3::MouseMode::Normal as i32,
            ..Default::default()
        };
        let env = build_mode_changed_envelope(rt, pn, 10, modes);

        let mut buf = BytesMut::new();
        encode_frame(&env, &mut buf).unwrap();
        let decoded: v3::ServerEnvelope = decode_frame(&mut buf).unwrap();
        assert_eq!(env, decoded);
    }

    #[test]
    fn mode_changed_envelope_contains_correct_payload() {
        let rt = uuid::Uuid::new_v4();
        let pn = uuid::Uuid::new_v4();
        let modes = v3::TerminalModeState {
            bracketed_paste: true,
            application_cursor_keys: true,
            alternate_screen: true,
            sgr_mouse: true,
            ..Default::default()
        };
        let env = build_mode_changed_envelope(rt, pn, 7, modes);

        match env.payload {
            Some(v3::server_envelope::Payload::TerminalModeChanged(ref changed)) => {
                assert_eq!(changed.runtime_id, uuid_to_bytes(rt));
                assert_eq!(changed.pane_id, uuid_to_bytes(pn));
                assert_eq!(changed.workspace_revision, 7);
                assert_eq!(changed.modes, Some(modes));
            }
            _ => panic!("expected TerminalModeChanged payload"),
        }
    }
}
