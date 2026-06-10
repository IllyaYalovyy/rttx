//! Integration test for #825: `StreamOverflow` recovery via resync.
//!
//! Verifies that `WorkspaceResynced` events produce correct snapshot
//! restores without rebuilding the layout, and that the resync path
//! handles multi-pane workspaces and terminal mode preservation.

use rttx::daemon_bridge::EndpointEvent;
use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy, WorkspaceRuntime};
use rttx::workspace::*;

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

fn pane_snapshot(
    pane_id: &str,
    title: &str,
    cwd: &str,
    scrollback: &[u8],
) -> rttx_proto::v3::PaneSnapshot {
    rttx_proto::v3::PaneSnapshot {
        pane_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(pane_id).unwrap()),
        pane_output_seq: 0,
        title: title.to_string(),
        cwd: cwd.to_string(),
        cols: 120,
        rows: 40,
        exit_status: None,
        terminal_modes: None,
        scrollback_tail: bytes::Bytes::copy_from_slice(scrollback),
        total_scrollback_bytes: scrollback.len() as u64,
        scrollback_complete: true,
    }
}

fn snapshot(
    runtime_id: &str,
    panes: Vec<rttx_proto::v3::PaneSnapshot>,
) -> rttx_proto::v3::RuntimeSnapshot {
    rttx_proto::v3::RuntimeSnapshot {
        tree: None,
        default_active_pane_id: Vec::new(),
        runtime_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(runtime_id).unwrap()),
        panes,
        runtime_revision: 1,
        client_role: rttx_proto::v3::RuntimeClientRole::Writer as i32,
    }
}

fn managed_workspace(id: &str, layout: LayoutNode, runtime_id: Option<&str>) -> WorkspaceState {
    let terminal_uuids = layout.terminal_uuids();
    let terminal_recovery = terminal_uuids
        .iter()
        .cloned()
        .map(|uuid| (uuid, rttx::workspace::PaneRecovery::empty_shell()))
        .collect();
    let active_terminal_uuid = terminal_uuids.first().cloned();
    let mut session = WorkspaceState {
        uuid: id.to_string(),
        name: id.to_string(),
        layout,
        terminal_recovery,
        active_terminal_uuid,
        input_sync: false,
        runtime: WorkspaceRuntime {
            managed: true,
            endpoint: RuntimeEndpoint::Local,
            policy: WorkspacePolicy::Persistent,
            runtime_id: runtime_id.map(str::to_string),
            pane_bindings: std::collections::BTreeMap::default(),
            pending_layout_panes: std::collections::BTreeSet::default(),
        },
        color: WorkspaceColor::default(),
        zoomed_terminal_uuid: None,
        user_renamed: false,
    };
    session.runtime.ensure_placeholder_bindings(&terminal_uuids);
    session
}

/// Resync on a multi-pane workspace restores all bound panes without
/// altering the layout structure.
#[test]
fn resync_multi_pane_workspace_restores_all_panes() {
    let runtime_id = uuid::Uuid::new_v4().to_string();
    let pane_a = uuid::Uuid::new_v4().to_string();
    let pane_b = uuid::Uuid::new_v4().to_string();

    let layout = hsplit(term(&pane_a), term(&pane_b));
    let mut state = WindowState {
        workspaces: vec![managed_workspace("ws-1", layout, None)],
        ..WindowState::default()
    };

    // Open workspace to establish bindings.
    let _ = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
        workspace_id: "ws-1".into(),
        runtime_id: runtime_id.clone(),
        snapshot: snapshot(
            &runtime_id,
            vec![
                pane_snapshot(&pane_a, "bash", "/home", b"initial-a"),
                pane_snapshot(&pane_b, "zsh", "/tmp", b"initial-b"),
            ],
        ),
    });

    let layout_before = state.workspaces[0].layout.terminal_uuids();

    // Resync with updated content.
    let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceResynced {
        workspace_id: "ws-1".into(),
        runtime_id: runtime_id.clone(),
        snapshot: snapshot(
            &runtime_id,
            vec![
                pane_snapshot(&pane_a, "bash", "/home/project", b"resynced-a"),
                pane_snapshot(&pane_b, "zsh", "/var/log", b"resynced-b"),
            ],
        ),
    });

    // Both panes should have snapshot restores.
    assert_eq!(transition.pane_snapshot_restores.len(), 2);

    let restore_a = transition
        .pane_snapshot_restores
        .iter()
        .find(|r| r.scrollback_tail == bytes::Bytes::from_static(b"resynced-a"))
        .expect("pane A should be restored");
    assert_eq!(restore_a.cwd, "/home/project");

    let restore_b = transition
        .pane_snapshot_restores
        .iter()
        .find(|r| r.scrollback_tail == bytes::Bytes::from_static(b"resynced-b"))
        .expect("pane B should be restored");
    assert_eq!(restore_b.cwd, "/var/log");

    // Layout must not change.
    assert_eq!(state.workspaces[0].layout.terminal_uuids(), layout_before);
    assert!(transition.rebuilt_workspaces.is_empty());
    assert!(transition.pane_create_requests.is_empty());
}

/// Repeated resyncs do not accumulate state or corrupt bindings.
#[test]
fn repeated_resyncs_are_idempotent() {
    let runtime_id = uuid::Uuid::new_v4().to_string();
    let pane_uuid = uuid::Uuid::new_v4().to_string();

    let mut state = WindowState {
        workspaces: vec![managed_workspace("ws-1", term(&pane_uuid), None)],
        ..WindowState::default()
    };

    let _ = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
        workspace_id: "ws-1".into(),
        runtime_id: runtime_id.clone(),
        snapshot: snapshot(
            &runtime_id,
            vec![pane_snapshot(&pane_uuid, "bash", "/home", b"initial")],
        ),
    });

    for i in 0..5 {
        let content = format!("resync-{i}");
        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceResynced {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(
                &runtime_id,
                vec![pane_snapshot(&pane_uuid, "bash", "/home", content.as_bytes())],
            ),
        });

        assert_eq!(transition.pane_snapshot_restores.len(), 1);
        assert_eq!(
            transition.pane_snapshot_restores[0].scrollback_tail,
            bytes::Bytes::from(content)
        );
        assert!(transition.rebuilt_workspaces.is_empty());
    }

    // Layout and bindings unchanged after 5 resyncs.
    assert_eq!(state.workspaces[0].layout.terminal_uuids().len(), 1);
    assert_eq!(state.workspaces[0].runtime.pane_bindings.len(), 1);
}
