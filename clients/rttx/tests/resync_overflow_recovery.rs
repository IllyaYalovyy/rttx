//! Integration test for #825: `StreamOverflow` recovery via resync.
//!
//! Verifies that `ResyncCompleted` events are correctly handled by the
//! workspace state reconciliation layer and that the snapshot-to-restore
//! mapping produces correct results for bound and unbound panes.

use rttx::daemon_bridge::EndpointEvent;
use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy, WorkspaceRuntime};
use rttx::workspace::*;
use rttx::workspace_state::WorkspacePaneRestore;
use std::collections::BTreeMap;

fn term(id: &str) -> LayoutNode {
    LayoutNode::Terminal { uuid: id.to_string(), profile: None, cwd: None, custom_title: None }
}

fn hsplit(first: LayoutNode, second: LayoutNode) -> LayoutNode {
    LayoutNode::Split {
        orientation: SplitOrientation::Horizontal,
        ratio: 0.5,
        first: Box::new(first),
        second: Box::new(second),
    }
}

fn pane_snapshot(pane_id: &str, title: &str, cwd: &str) -> rttx_proto::v3::PaneSnapshot {
    rttx_proto::v3::PaneSnapshot {
        pane_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(pane_id).unwrap()),
        pane_output_seq: 200,
        title: title.to_string(),
        cwd: cwd.to_string(),
        cols: 80,
        rows: 24,
        exit_status: None,
        terminal_modes: None,
        scrollback_tail: bytes::Bytes::from_static(b"$ echo resynced\r\n"),
        total_scrollback_bytes: 17,
        scrollback_complete: true,
    }
}

fn snapshot(
    runtime_id: &str,
    panes: Vec<rttx_proto::v3::PaneSnapshot>,
) -> rttx_proto::v3::RuntimeSnapshot {
    rttx_proto::v3::RuntimeSnapshot {
        runtime_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(runtime_id).unwrap()),
        panes,
        runtime_revision: 42,
        client_role: rttx_proto::v3::RuntimeClientRole::Writer as i32,
    }
}

/// `ResyncCompleted` is a no-op in `reconcile_endpoint_event` — it does not
/// mutate workspace state. The snapshot application happens in the window
/// layer, not the state layer.
#[test]
fn resync_completed_does_not_mutate_workspace_state() {
    let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
    let pane_id_1 = "a1a1a1a1-b2b2-c3c3-d4d4-e5e5e5e5e5e5";

    let mut session =
        WorkspaceState::new_managed_local("Home".into(), WorkspacePolicy::Persistent, None);
    session.uuid = "ws-1".into();
    session.layout = term("left");
    session.runtime = WorkspaceRuntime::managed_local(
        WorkspacePolicy::Persistent,
        &session.layout.terminal_uuids(),
    );
    session.runtime.runtime_id = Some(runtime_id.into());
    session.runtime.pane_bindings = BTreeMap::from([("left".into(), pane_id_1.into())]);

    let mut state = WindowState {
        active_workspace_index: 0,
        workspaces: vec![session],
        ..WindowState::default()
    };

    let state_before = state.workspaces[0].clone();

    let snap = snapshot(runtime_id, vec![pane_snapshot(pane_id_1, "bash", "/tmp")]);
    let transition = state.reconcile_endpoint_event(&EndpointEvent::ResyncCompleted {
        endpoint: RuntimeEndpoint::Local,
        runtime_id: runtime_id.into(),
        snapshot: snap,
        dropped_count: 5,
    });

    // State must be unchanged — resync is handled in the window layer.
    assert_eq!(state.workspaces[0], state_before);
    assert!(transition.pane_snapshot_restores.is_empty());
    assert!(transition.rebuilt_workspaces.is_empty());
    assert!(transition.recovered_workspaces.is_empty());
}

/// Verify that a resync snapshot can be decomposed into the correct
/// `WorkspacePaneRestore` structs for each bound pane.
#[test]
fn resync_snapshot_maps_to_pane_restores() {
    let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
    let pane_id_1 = "a1a1a1a1-b2b2-c3c3-d4d4-e5e5e5e5e5e5";
    let pane_id_2 = "b2b2b2b2-c3c3-d4d4-e5e5-f6f6f6f6f6f6";

    let mut session =
        WorkspaceState::new_managed_local("Home".into(), WorkspacePolicy::Persistent, None);
    session.uuid = "ws-1".into();
    session.layout = hsplit(term("left"), term("right"));
    session.runtime = WorkspaceRuntime::managed_local(
        WorkspacePolicy::Persistent,
        &session.layout.terminal_uuids(),
    );
    session.runtime.runtime_id = Some(runtime_id.into());
    session.runtime.pane_bindings =
        BTreeMap::from([("left".into(), pane_id_1.into()), ("right".into(), pane_id_2.into())]);

    let snap = snapshot(
        runtime_id,
        vec![pane_snapshot(pane_id_1, "bash", "/home"), pane_snapshot(pane_id_2, "vim", "/tmp")],
    );

    // Simulate the mapping logic from apply_resync_snapshot.
    let bindings = &session.runtime.pane_bindings;
    let mut restores = Vec::new();
    for pane_snap in &snap.panes {
        let pane_uuid = rttx_proto::bytes_to_uuid(&pane_snap.pane_id).unwrap();
        let runtime_pane_id = pane_uuid.to_string();
        if let Some((layout_uuid, _)) = bindings.iter().find(|(_, rpid)| **rpid == runtime_pane_id)
        {
            restores.push(WorkspacePaneRestore {
                layout_terminal_uuid: layout_uuid.clone(),
                title: pane_snap.title.clone(),
                cwd: pane_snap.cwd.clone(),
                pane_output_seq: pane_snap.pane_output_seq,
                scrollback_tail: pane_snap.scrollback_tail.clone(),
                scrollback_complete: pane_snap.scrollback_complete,
                cols: pane_snap.cols as u16,
                rows: pane_snap.rows as u16,
                terminal_modes: pane_snap.terminal_modes,
            });
        }
    }

    assert_eq!(restores.len(), 2);
    assert_eq!(restores[0].layout_terminal_uuid, "left");
    assert_eq!(restores[0].cwd, "/home");
    assert_eq!(restores[0].title, "bash");
    assert_eq!(restores[1].layout_terminal_uuid, "right");
    assert_eq!(restores[1].cwd, "/tmp");
    assert_eq!(restores[1].title, "vim");
}

/// Unbound panes in the snapshot (e.g. panes created after the last
/// reconciliation) are silently skipped during resync.
#[test]
fn resync_snapshot_skips_unbound_panes() {
    let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
    let pane_id_1 = "a1a1a1a1-b2b2-c3c3-d4d4-e5e5e5e5e5e5";
    let unbound_pane = "cccccccc-dddd-eeee-ffff-000000000000";

    let mut session =
        WorkspaceState::new_managed_local("Home".into(), WorkspacePolicy::Persistent, None);
    session.uuid = "ws-1".into();
    session.layout = term("left");
    session.runtime = WorkspaceRuntime::managed_local(
        WorkspacePolicy::Persistent,
        &session.layout.terminal_uuids(),
    );
    session.runtime.runtime_id = Some(runtime_id.into());
    session.runtime.pane_bindings = BTreeMap::from([("left".into(), pane_id_1.into())]);

    let snap = snapshot(
        runtime_id,
        vec![
            pane_snapshot(pane_id_1, "bash", "/home"),
            pane_snapshot(unbound_pane, "orphan", "/tmp"),
        ],
    );

    let bindings = &session.runtime.pane_bindings;
    let mut restores = Vec::new();
    for pane_snap in &snap.panes {
        let pane_uuid = rttx_proto::bytes_to_uuid(&pane_snap.pane_id).unwrap();
        let runtime_pane_id = pane_uuid.to_string();
        if let Some((layout_uuid, _)) = bindings.iter().find(|(_, rpid)| **rpid == runtime_pane_id)
        {
            restores.push(layout_uuid.clone());
        }
    }

    assert_eq!(restores.len(), 1);
    assert_eq!(restores[0], "left");
}
