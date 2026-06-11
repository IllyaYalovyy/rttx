//! Integration coverage for the one-time orphaned-histfile salvage utility
//! (RFC-031 §9 / Step 7, issue #1004).
//!
//! Builds a synthetic daemon state directory that mirrors a real upgrade: one
//! current-schema workspace that still has a live pane plus a stale history file
//! left by the pre-RFC-031 random-pane-id bug, and one old-schema workspace whose
//! `workspace.json` references no current panes. The salvage scan must find every
//! orphan, ignore the live pane's history, and export recovered files into a
//! separate recovery directory without disturbing the source state.

use rttx_server::pane_tree::{PaneId, WorkspaceTree};
use rttx_server::salvage::{export_orphans, scan_orphans};
use rttx_server::state::layout;
use rttx_server::state::persistence::{save_daemon_index, save_workspace};
use rttx_server::state::types::{
    PaneSpecV2, RUNTIME_FILE_SCHEMA_VERSION, WorkspaceFileV2, WorkspaceInstanceV1, WorkspaceSpecV2,
};
use rttx_server::workspace::WorkspacePolicy;
use std::path::Path;
use std::time::SystemTime;
use uuid::Uuid;

fn write_hist(state_dir: &Path, runtime_id: Uuid, pane_id: Uuid, contents: &str) {
    let path = layout::history_file(state_dir, runtime_id, pane_id);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, contents).unwrap();
}

/// Persist a current-schema workspace with a single live pane.
fn persist_current_workspace(state_dir: &Path, runtime_id: Uuid, live: PaneId) {
    let mut tree = WorkspaceTree::new();
    tree.insert_root(live);
    let workspace = WorkspaceFileV2 {
        schema_version: RUNTIME_FILE_SCHEMA_VERSION,
        spec: WorkspaceSpecV2 {
            id: runtime_id,
            name: "upgraded".into(),
            policy: WorkspacePolicy::Persistent,
            created_at: SystemTime::now(),
            tree,
            panes: vec![PaneSpecV2 {
                id: live,
                cwd: Some("/home/user/project".into()),
                title: Some("bash".into()),
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
    save_workspace(state_dir, &workspace).unwrap();
}

#[test]
fn salvage_recovers_every_orphan_without_touching_live_state() {
    let tmp = tempfile::TempDir::new().unwrap();
    let state_dir = tmp.path().join("state/rttx/daemon");

    // Workspace A: current schema, one live pane plus one stale orphan from the
    // pre-refactor random-id bug.
    let rt_a = Uuid::new_v4();
    let live = PaneId::new();
    persist_current_workspace(&state_dir, rt_a, live);
    write_hist(&state_dir, rt_a, live.uuid(), "echo still here\n");
    let stale = Uuid::new_v4();
    write_hist(&state_dir, rt_a, stale, "echo orphaned by reconnect\n");

    // Workspace B: old-schema workspace.json (clean-break) with two history files
    // that the daemon would otherwise discard on first start.
    let rt_b = Uuid::new_v4();
    let rt_b_file = layout::runtime_file(&state_dir, rt_b);
    std::fs::create_dir_all(rt_b_file.parent().unwrap()).unwrap();
    std::fs::write(&rt_b_file, r#"{"schema_version":1,"spec":{},"instance":{}}"#).unwrap();
    let old1 = Uuid::new_v4();
    let old2 = Uuid::new_v4();
    write_hist(&state_dir, rt_b, old1, "fg\n");
    write_hist(&state_dir, rt_b, old2, "make release\n");

    save_daemon_index(&state_dir, &[rt_a, rt_b]).unwrap();

    // Scan: three orphans (stale in A, old1 + old2 in B); the live pane is not
    // reported.
    let orphans = scan_orphans(&state_dir);
    let recovered: std::collections::BTreeSet<Uuid> = orphans.iter().map(|o| o.pane_id).collect();
    assert_eq!(orphans.len(), 3, "found: {orphans:?}");
    assert!(recovered.contains(&stale));
    assert!(recovered.contains(&old1));
    assert!(recovered.contains(&old2));
    assert!(!recovered.contains(&live.uuid()), "the live pane's history must not be salvaged");

    // Export into a recovery directory outside workspaces/.
    let dest = tmp.path().join("recovery");
    let report = export_orphans(&orphans, &dest).unwrap();
    assert_eq!(report.exported.len(), 3);

    assert_eq!(
        std::fs::read_to_string(dest.join(rt_a.to_string()).join(format!("{stale}.hist"))).unwrap(),
        "echo orphaned by reconnect\n"
    );
    assert_eq!(
        std::fs::read_to_string(dest.join(rt_b.to_string()).join(format!("{old2}.hist"))).unwrap(),
        "make release\n"
    );

    // Source state is untouched: the live history and the orphan sources remain.
    assert!(layout::history_file(&state_dir, rt_a, live.uuid()).exists());
    assert!(layout::history_file(&state_dir, rt_a, stale).exists());
    assert!(layout::history_file(&state_dir, rt_b, old1).exists());
}

#[test]
fn salvage_reports_nothing_for_a_clean_install() {
    let tmp = tempfile::TempDir::new().unwrap();
    let state_dir = tmp.path().join("state/rttx/daemon");
    let rt = Uuid::new_v4();
    let live = PaneId::new();
    persist_current_workspace(&state_dir, rt, live);
    write_hist(&state_dir, rt, live.uuid(), "echo hello\n");
    save_daemon_index(&state_dir, &[rt]).unwrap();

    assert!(
        scan_orphans(&state_dir).is_empty(),
        "a state dir with only referenced history has nothing to salvage"
    );
}
