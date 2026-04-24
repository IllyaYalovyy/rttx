//! Property-based robustness tests for `PaneScreen` terminal parser and
//! snapshot/state deserialization under corrupted input.
//!
//! Validates that the terminal parser never panics, never enters unbounded
//! growth, and that corrupted JSON state files produce errors rather than
//! panics.

use proptest::prelude::*;
use rttx_server::screen::{restart_safe_scrollback, strip_client_queries, PaneScreen};
use rttx_server::serialization::{load_state, ServerState};
use rttx_server::state::types::{
    DaemonIndexV1, RuntimeFileV1, ScreenSnapshotV1, SchemaVersionEnvelope,
};
use tempfile::TempDir;

// ── PaneScreen: arbitrary byte streams never panic ──────────────────

proptest! {
    #[test]
    fn pane_screen_feed_never_panics(data in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let mut screen = PaneScreen::new(8192);
        screen.feed(&data);
    }

    #[test]
    fn pane_screen_feed_raw_bytes_bounded(data in proptest::collection::vec(any::<u8>(), 0..8192)) {
        let max = 1024;
        let mut screen = PaneScreen::new(max);
        screen.feed(&data);
        prop_assert!(screen.raw_bytes().len() <= max);
    }

    #[test]
    fn pane_screen_snapshot_bytes_bounded(
        data in proptest::collection::vec(any::<u8>(), 0..4096),
        cap in 1..2048usize,
    ) {
        let mut screen = PaneScreen::new(8192);
        screen.feed(&data);
        let snap = screen.snapshot_bytes(cap);
        prop_assert!(snap.len() <= cap);
    }

    #[test]
    fn pane_screen_incremental_feed_equivalent_to_bulk(
        chunks in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..256), 1..8),
    ) {
        let all: Vec<u8> = chunks.iter().flatten().copied().collect();

        let mut bulk = PaneScreen::new(16384);
        bulk.feed(&all);

        let mut incremental = PaneScreen::new(16384);
        for chunk in &chunks {
            incremental.feed(chunk);
        }

        prop_assert_eq!(bulk.raw_bytes(), incremental.raw_bytes());
        prop_assert_eq!(bulk.cursor_position(), incremental.cursor_position());
        prop_assert_eq!(bulk.bracketed_paste_mode(), incremental.bracketed_paste_mode());
        prop_assert_eq!(bulk.application_cursor_keys(), incremental.application_cursor_keys());
        prop_assert_eq!(bulk.application_keypad(), incremental.application_keypad());
        prop_assert_eq!(bulk.mouse_tracking_mode(), incremental.mouse_tracking_mode());
        prop_assert_eq!(bulk.sgr_mouse_mode(), incremental.sgr_mouse_mode());
        prop_assert_eq!(bulk.focus_event_mode(), incremental.focus_event_mode());
        prop_assert_eq!(bulk.cursor_visible(), incremental.cursor_visible());
        prop_assert_eq!(bulk.alternate_screen(), incremental.alternate_screen());
    }

    #[test]
    fn pane_screen_pending_replies_bounded(data in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let mut screen = PaneScreen::new(8192);
        screen.feed(&data);
        let replies = screen.take_pending_replies();
        // Each reply is a response to a specific query sequence; the number
        // of replies cannot exceed the number of bytes fed.
        prop_assert!(replies.len() <= data.len());
    }
}

// ── strip_client_queries: never panics, output ≤ input ──────────────

proptest! {
    #[test]
    fn strip_client_queries_never_panics(data in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let result = strip_client_queries(&data);
        prop_assert!(result.len() <= data.len());
    }
}

// ── restart_safe_scrollback: never panics, output ≤ input ───────────

proptest! {
    #[test]
    fn restart_safe_scrollback_never_panics(data in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let result = restart_safe_scrollback(&data);
        prop_assert!(result.len() <= data.len());
    }

    #[test]
    fn restart_safe_scrollback_ends_at_line_boundary(data in proptest::collection::vec(any::<u8>(), 1..2048)) {
        let result = restart_safe_scrollback(&data);
        if !result.is_empty() {
            let last = *result.last().unwrap();
            prop_assert!(last == b'\n' || last == b'\r');
        }
    }
}

// ── Snapshot/state deserialization: corrupted JSON never panics ──────

proptest! {
    #[test]
    fn load_state_corrupt_json_never_panics(data in proptest::collection::vec(any::<u8>(), 0..1024)) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("state.json");
        std::fs::write(&path, &data).unwrap();
        let _ = load_state(&path);
    }

    #[test]
    fn daemon_index_corrupt_json_never_panics(data in "\\PC{0,512}") {
        let _: Result<DaemonIndexV1, _> = serde_json::from_str(&data);
    }

    #[test]
    fn runtime_file_corrupt_json_never_panics(data in "\\PC{0,512}") {
        let _: Result<RuntimeFileV1, _> = serde_json::from_str(&data);
    }

    #[test]
    fn screen_snapshot_corrupt_json_never_panics(data in "\\PC{0,512}") {
        let _: Result<ScreenSnapshotV1, _> = serde_json::from_str(&data);
    }

    #[test]
    fn schema_envelope_corrupt_json_never_panics(data in "\\PC{0,512}") {
        let _: Result<SchemaVersionEnvelope, _> = serde_json::from_str(&data);
    }

    #[test]
    fn server_state_corrupt_json_never_panics(data in "\\PC{0,512}") {
        let _: Result<ServerState, _> = serde_json::from_str(&data);
    }
}

// ── Snapshot load with field mutation ───────────────────────────────

proptest! {
    #[test]
    fn screen_snapshot_with_arbitrary_screen_bytes(
        screen_bytes in proptest::collection::vec(any::<u8>(), 0..2048),
        cols in 1u16..500,
        rows in 1u16..200,
    ) {
        let snap = ScreenSnapshotV1 {
            schema_version: 1,
            pane_id: uuid::Uuid::new_v4(),
            cols,
            rows,
            cursor_row: 0,
            cursor_col: 0,
            cursor_visible: true,
            title: None,
            cwd: None,
            pane_output_seq: 0,
            modes: rttx_server::state::types::TerminalModeSnapshot {
                bracketed_paste: false,
                application_cursor_keys: false,
                application_keypad: false,
                mouse_tracking_mode: 0,
                sgr_mouse: false,
                focus_reporting: false,
            },
            screen_bytes: screen_bytes.clone(),
            confidential: false,
        };
        let json = serde_json::to_string(&snap).unwrap();
        let recovered: ScreenSnapshotV1 = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(&recovered.screen_bytes, &screen_bytes);

        // Feed the recovered screen bytes into PaneScreen — must not panic.
        let mut pane_screen = PaneScreen::new(8192);
        pane_screen.feed(&recovered.screen_bytes);
    }
}

// ── State file: truncated valid JSON ────────────────────────────────

proptest! {
    #[test]
    fn load_state_truncated_valid_json_never_panics(cut in 0..200usize) {
        let state = ServerState {
            runtimes: vec![],
            serialized_at: std::time::SystemTime::now(),
            server_version: "test".into(),
        };
        let json = serde_json::to_string_pretty(&state).unwrap();
        let cut = cut.min(json.len());
        let truncated = &json[..cut];

        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("state.json");
        std::fs::write(&path, truncated).unwrap();
        let _ = load_state(&path);
    }
}
