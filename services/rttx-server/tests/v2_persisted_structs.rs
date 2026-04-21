//! Integration test for v2 persisted structs and migration chain (RFC-022 §2–§4).
//!
//! Verifies that structs serialize to disk and load back through the
//! migration chain, including future-version rejection and forward
//! compatibility with unknown fields.

use rttx_server::runtime::RuntimePolicy;
use rttx_server::state::migrations::{
    load_daemon_index, load_runtime_file, load_screen_snapshot, peek_schema_version,
};
use rttx_server::state::types::*;
use std::time::SystemTime;
use tempfile::TempDir;
use uuid::Uuid;

fn write_and_load_daemon_index(index: &DaemonIndexV1) -> DaemonIndexV1 {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("daemon.json");
    let json = serde_json::to_string_pretty(index).unwrap();
    std::fs::write(&path, &json).unwrap();
    let loaded = std::fs::read_to_string(&path).unwrap();
    load_daemon_index(&loaded).unwrap()
}

fn write_and_load_runtime_file(file: &RuntimeFileV1) -> RuntimeFileV1 {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("runtime.json");
    let json = serde_json::to_string_pretty(file).unwrap();
    std::fs::write(&path, &json).unwrap();
    let loaded = std::fs::read_to_string(&path).unwrap();
    load_runtime_file(&loaded).unwrap()
}

fn write_and_load_screen_snapshot(snap: &ScreenSnapshotV1) -> ScreenSnapshotV1 {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("screen.snap");
    let json = serde_json::to_string_pretty(snap).unwrap();
    std::fs::write(&path, &json).unwrap();
    let loaded = std::fs::read_to_string(&path).unwrap();
    load_screen_snapshot(&loaded).unwrap()
}

#[test]
fn daemon_index_persists_and_loads_via_migration_chain() {
    let original = DaemonIndexV1 {
        schema_version: DAEMON_INDEX_SCHEMA_VERSION,
        server_version: "0.4.0".into(),
        runtime_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
        created_at: SystemTime::now(),
        last_serialized_at: SystemTime::now(),
    };
    let recovered = write_and_load_daemon_index(&original);
    assert_eq!(original, recovered);
}

#[test]
fn runtime_file_persists_and_loads_via_migration_chain() {
    let pane = PaneSpecV1 {
        id: Uuid::new_v4(),
        cwd: Some("/home/user/project".into()),
        title: Some("nvim".into()),
        exit_status: None,
        cols: 120,
        rows: 40,
    };
    let original = RuntimeFileV1 {
        schema_version: RUNTIME_FILE_SCHEMA_VERSION,
        spec: RuntimeSpecV1 {
            id: Uuid::new_v4(),
            name: "workspace-1".into(),
            policy: RuntimePolicy::Persistent,
            created_at: SystemTime::now(),
            panes: vec![pane],
            active_pane_id: None,
            command_history: vec![HistoryEntryV1 {
                command: "cargo build".into(),
                cwd: "/home/user/project".into(),
                timestamp: SystemTime::now(),
                pane_id: Uuid::new_v4(),
            }],
        },
        instance: RuntimeInstanceV1 {
            revision: 7,
            last_active_at: SystemTime::now(),
            last_snapshot_at: SystemTime::now(),
        },
    };
    let recovered = write_and_load_runtime_file(&original);
    assert_eq!(original, recovered);
}

#[test]
fn screen_snapshot_persists_and_loads_via_migration_chain() {
    let original = ScreenSnapshotV1 {
        schema_version: SCREEN_SNAPSHOT_SCHEMA_VERSION,
        pane_id: Uuid::new_v4(),
        cols: 80,
        rows: 24,
        cursor_row: 12,
        cursor_col: 40,
        cursor_visible: true,
        title: Some("bash".into()),
        cwd: Some("/tmp".into()),
        pane_output_seq: 500,
        modes: TerminalModeSnapshot {
            bracketed_paste: true,
            application_cursor_keys: false,
            application_keypad: false,
            mouse_tracking_mode: 1000,
            sgr_mouse: true,
            focus_reporting: false,
        },
        screen_bytes: vec![0x1b, b'[', b'2', b'J'],
    };
    let recovered = write_and_load_screen_snapshot(&original);
    assert_eq!(original, recovered);
}

#[test]
fn forward_compatible_ignores_unknown_fields() {
    // Simulate a future writer adding an extra field — serde should ignore it.
    let json = r#"{
        "schema_version": 1,
        "server_version": "0.5.0",
        "runtime_ids": [],
        "created_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
        "last_serialized_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
        "future_field": "should be ignored"
    }"#;
    let version = peek_schema_version(json).unwrap();
    assert_eq!(version, 1);
    // Should load successfully despite the unknown field.
    let result = load_daemon_index(json);
    assert!(result.is_ok());
}

#[test]
fn future_version_rejected_from_disk() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("daemon.json");
    let json = r#"{"schema_version": 99, "server_version": "9.0.0", "runtime_ids": []}"#;
    std::fs::write(&path, json).unwrap();
    let loaded = std::fs::read_to_string(&path).unwrap();
    let result = load_daemon_index(&loaded);
    assert!(result.is_err());
}
