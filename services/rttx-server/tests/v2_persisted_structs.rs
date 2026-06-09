//! Integration tests for persisted state structs and the clean-break loader
//! (RFC-022 §2–§4, RFC-031 §6).
//!
//! Verifies that the daemon index and screen snapshots round-trip through the
//! migration chain, that the durable `WorkspaceFileV2` (tree + panes) round-trips
//! through the persistence layer, and that old-schema runtime state is detected,
//! ignored, and removed with no migration path.

use rttx_server::pane_tree::{PaneId, SplitAxis, WorkspaceTree};
use rttx_server::runtime::RuntimePolicy;
use rttx_server::state::migrations::{load_daemon_index, load_screen_snapshot, peek_schema_version};
use rttx_server::state::types::*;
use rttx_server::state::{layout, persistence};
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
fn workspace_file_persists_and_reloads_with_tree() {
    let tmp = TempDir::new().unwrap();
    let state_dir = tmp.path();
    let rt_id = Uuid::new_v4();

    // A two-level tree with distinct ratios and a non-default active pane.
    let (a, b, c) = (PaneId::new(), PaneId::new(), PaneId::new());
    let mut tree = WorkspaceTree::new();
    tree.insert_root(a);
    tree.split(a, b, SplitAxis::Horizontal, 0.3);
    tree.split(b, c, SplitAxis::Vertical, 0.6);
    tree.set_default_active(b);
    let expected_tree = tree.clone();

    let mk = |id: PaneId, title: &str| PaneSpecV2 {
        id,
        cwd: Some("/home/user/project".into()),
        title: Some(title.into()),
        exit_status: None,
        cols: 120,
        rows: 40,
        no_persist: false,
    };
    let original = WorkspaceFileV2 {
        schema_version: RUNTIME_FILE_SCHEMA_VERSION,
        spec: WorkspaceSpecV2 {
            id: rt_id,
            name: "workspace-1".into(),
            policy: RuntimePolicy::Persistent,
            created_at: SystemTime::now(),
            tree,
            panes: vec![mk(a, "bash"), mk(b, "nvim"), mk(c, "logs")],
        },
        instance: RuntimeInstanceV1 {
            revision: 7,
            last_active_at: SystemTime::now(),
            last_snapshot_at: SystemTime::now(),
        },
    };

    persistence::save_daemon_index(state_dir, &[rt_id]).unwrap();
    persistence::save_runtime(state_dir, &original).unwrap();

    let result = persistence::load_all(state_dir).unwrap();
    assert!(result.failed_ids.is_empty());
    assert!(result.reset_ids.is_empty());
    assert_eq!(result.runtimes.len(), 1);

    let recovered = &result.runtimes[0];
    assert_eq!(recovered.spec.tree, expected_tree, "tree structure + ratios must survive");
    assert_eq!(recovered.spec.tree.default_active(), Some(b));
    assert_eq!(recovered.spec.panes.len(), 3);
    assert_eq!(recovered.instance.revision, 7);
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
        confidential: false,
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

#[test]
fn old_schema_runtime_file_is_reset_not_migrated() {
    // RFC-031 clean break: an old v1 runtime.json (flat panes, active_pane_id,
    // command_history, no durable tree) is detected, ignored, and removed.
    let tmp = TempDir::new().unwrap();
    let state_dir = tmp.path();
    let old_id = Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();

    let old_dir = layout::runtime_dir(state_dir, old_id);
    std::fs::create_dir_all(&old_dir).unwrap();
    let v1_json = r#"{
        "schema_version": 1,
        "spec": {
            "id": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            "name": "legacy-ws",
            "policy": "persistent",
            "created_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
            "panes": [{
                "id": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
                "cwd": "/home/user",
                "title": "bash",
                "exit_status": null,
                "cols": 80,
                "rows": 24
            }],
            "active_pane_id": null,
            "command_history": [
                {"command": "cargo build", "cwd": "/home/user/project"}
            ]
        },
        "instance": {
            "revision": 5,
            "last_active_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
            "last_snapshot_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0}
        }
    }"#;
    std::fs::write(layout::runtime_file(state_dir, old_id), v1_json).unwrap();
    persistence::save_daemon_index(state_dir, &[old_id]).unwrap();

    let result = persistence::load_all(state_dir).unwrap();
    assert!(result.runtimes.is_empty(), "old-schema runtime must not load");
    assert!(result.failed_ids.is_empty(), "old schema is a reset, not a failure");
    assert_eq!(result.reset_ids, vec![old_id]);
    assert!(!old_dir.exists(), "old-schema runtime directory must be removed on load");
}
