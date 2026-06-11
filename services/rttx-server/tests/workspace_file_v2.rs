//! Behavior tests for `WorkspaceFileV2` durable persistence and the clean-break
//! loader (RFC-031 Step 2).
//!
//! These exercise the public persistence API end-to-end: the authoritative pane
//! tree survives a save/load round trip, and old-schema (v1) state is detected,
//! ignored, and removed on load with no migration path.

use rttx_server::pane_tree::{PaneId, SplitAxis, WorkspaceTree};
use rttx_server::state::types::{
    PaneSpecV2, RUNTIME_FILE_SCHEMA_VERSION, WorkspaceFileV2, WorkspaceInstanceV1, WorkspaceSpecV2,
};
use rttx_server::state::{layout, persistence};
use rttx_server::workspace::WorkspacePolicy;
use std::time::SystemTime;
use tempfile::TempDir;
use uuid::Uuid;

#[test]
fn multi_pane_tree_survives_save_and_load() {
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
            policy: WorkspacePolicy::Persistent,
            created_at: SystemTime::now(),
            tree,
            panes: vec![mk(a, "bash"), mk(b, "nvim"), mk(c, "logs")],
        },
        instance: WorkspaceInstanceV1 {
            revision: 7,
            last_active_at: SystemTime::now(),
            last_snapshot_at: SystemTime::now(),
        },
    };

    persistence::save_daemon_index(state_dir, &[rt_id]).unwrap();
    persistence::save_workspace(state_dir, &original).unwrap();

    let result = persistence::load_all(state_dir).unwrap();
    assert!(result.failed_ids.is_empty());
    assert!(result.reset_ids.is_empty());
    assert_eq!(result.workspaces.len(), 1);

    let recovered = &result.workspaces[0];
    assert_eq!(recovered.spec.tree, expected_tree, "tree structure + ratios must survive");
    assert_eq!(recovered.spec.tree.default_active(), Some(b));
    assert_eq!(recovered.spec.panes.len(), 3);
    assert_eq!(recovered.instance.revision, 7);
}

#[test]
fn old_schema_runtime_file_is_reset_not_migrated() {
    // RFC-031 clean break: an old v1 workspace.json (flat panes, active_pane_id,
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
    assert!(result.workspaces.is_empty(), "old-schema workspace must not load");
    assert!(result.failed_ids.is_empty(), "old schema is a reset, not a failure");
    assert_eq!(result.reset_ids, vec![old_id]);
    assert!(!old_dir.exists(), "old-schema workspace directory must be removed on load");
}

#[test]
fn good_v2_workspace_survives_alongside_old_schema_sibling() {
    let tmp = TempDir::new().unwrap();
    let state_dir = tmp.path();
    let good_id = Uuid::new_v4();
    let old_id = Uuid::new_v4();

    // A current-schema workspace.
    let pane = PaneId::new();
    let mut tree = WorkspaceTree::new();
    tree.insert_root(pane);
    let good = WorkspaceFileV2 {
        schema_version: RUNTIME_FILE_SCHEMA_VERSION,
        spec: WorkspaceSpecV2 {
            id: good_id,
            name: "current".into(),
            policy: WorkspacePolicy::Persistent,
            created_at: SystemTime::now(),
            tree,
            panes: vec![PaneSpecV2 {
                id: pane,
                cwd: None,
                title: None,
                exit_status: None,
                cols: 80,
                rows: 24,
                no_persist: false,
            }],
        },
        instance: WorkspaceInstanceV1 {
            revision: 1,
            last_active_at: SystemTime::now(),
            last_snapshot_at: SystemTime::now(),
        },
    };
    persistence::save_workspace(state_dir, &good).unwrap();

    // A stale v1 sibling.
    let old_dir = layout::runtime_dir(state_dir, old_id);
    std::fs::create_dir_all(&old_dir).unwrap();
    std::fs::write(
        layout::runtime_file(state_dir, old_id),
        r#"{"schema_version": 1, "spec": {}, "instance": {}}"#,
    )
    .unwrap();

    persistence::save_daemon_index(state_dir, &[good_id, old_id]).unwrap();

    let result = persistence::load_all(state_dir).unwrap();
    assert_eq!(result.workspaces.len(), 1);
    assert_eq!(result.workspaces[0].spec.id, good_id);
    assert_eq!(result.reset_ids, vec![old_id]);
    assert!(!old_dir.exists());
}
