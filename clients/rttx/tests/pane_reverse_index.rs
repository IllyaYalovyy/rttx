//! Integration tests for the pane reverse index.
//!
//! Verifies that `runtime_pane_target()` returns correct results through
//! full workspace lifecycle scenarios: create, bind, reconcile, close.

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

fn managed_session(
    id: &str,
    name: &str,
    layout: LayoutNode,
    endpoint: RuntimeEndpoint,
    policy: WorkspacePolicy,
    runtime_id: Option<&str>,
) -> WorkspaceState {
    let terminal_uuids = layout.terminal_uuids();
    let terminal_recovery =
        terminal_uuids.iter().cloned().map(|uuid| (uuid, PaneRecovery::empty_shell())).collect();
    let active_terminal_uuid = terminal_uuids.first().cloned();
    WorkspaceState {
        uuid: id.to_string(),
        name: name.to_string(),
        layout,
        terminal_recovery,
        active_terminal_uuid,
        input_sync: false,
        runtime: WorkspaceRuntime {
            managed: true,
            endpoint,
            policy,
            runtime_id: runtime_id.map(str::to_string),
        },
        color: WorkspaceColor::default(),
        zoomed_terminal_uuid: None,
        user_renamed: false,
    }
}

fn window_state(workspaces: Vec<WorkspaceState>) -> WindowState {
    let mut state = WindowState { workspaces, active_workspace_index: 0, ..WindowState::default() };
    state.rebuild_pane_reverse_index();
    state
}

fn pane_snapshot(pane_id: &str, title: &str, cwd: &str) -> rttx_proto::v3::PaneSnapshot {
    rttx_proto::v3::PaneSnapshot {
        pane_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(pane_id).unwrap()),
        pane_output_seq: 0,
        title: title.to_string(),
        cwd: cwd.to_string(),
        cols: 120,
        rows: 40,
        exit_status: None,
        terminal_modes: None,
        scrollback_tail: bytes::Bytes::new(),
        total_scrollback_bytes: 0,
        scrollback_complete: true,
    }
}

fn snapshot(
    runtime_id: &str,
    panes: Vec<rttx_proto::v3::PaneSnapshot>,
) -> rttx_proto::v3::WorkspaceSnapshot {
    rttx_proto::v3::WorkspaceSnapshot {
        tree: None,
        default_active_pane_id: Vec::new(),
        runtime_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(runtime_id).unwrap()),
        panes,
        workspace_revision: 1,
        client_role: rttx_proto::v3::WorkspaceClientRole::Writer as i32,
    }
}

/// Full lifecycle: create workspace → bind panes → close pane → verify index.
#[test]
fn lifecycle_create_bind_close_keeps_index_consistent() {
    let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
    let pane_a = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
    let pane_b = "0d88f17f-626d-40b8-a1d3-6a42af628ac9";

    let mut state = window_state(vec![managed_session(
        "ws-1",
        "Workspace",
        hsplit(term("left"), term("right")),
        RuntimeEndpoint::Local,
        WorkspacePolicy::Persistent,
        None,
    )]);

    // Before binding: no lookups should resolve to the server pane ids.
    assert!(state.runtime_pane_target(&RuntimeEndpoint::Local, pane_a).is_none());

    // Bind panes via apply_managed_pane_created (re-keys layout to pane ids).
    state.apply_managed_pane_created("ws-1", "left", runtime_id, pane_a);
    state.apply_managed_pane_created("ws-1", "right", runtime_id, pane_b);

    // After the re-key each layout terminal IS its server pane id (identity).
    assert_eq!(
        state.runtime_pane_target(&RuntimeEndpoint::Local, pane_a),
        Some(("ws-1".into(), pane_a.into())),
    );
    assert_eq!(
        state.runtime_pane_target(&RuntimeEndpoint::Local, pane_b),
        Some(("ws-1".into(), pane_b.into())),
    );

    // Close one pane (identity: close by its server pane id).
    state.apply_managed_pane_closed("ws-1", pane_a);

    // Closed pane should no longer resolve.
    assert!(state.runtime_pane_target(&RuntimeEndpoint::Local, pane_a).is_none());
    // Remaining pane should still resolve.
    assert_eq!(
        state.runtime_pane_target(&RuntimeEndpoint::Local, pane_b),
        Some(("ws-1".into(), pane_b.into())),
    );
}

/// A daemon restart delivers a new authoritative tree; the client adopts it
/// and the index reflects the new (identity) pane ids.
#[test]
fn reconciliation_updates_index_after_daemon_restart() {
    use rttx_proto::v3_tree::pane_tree_leaf;

    let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
    let old_pane = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
    let new_pane = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";

    // The layout terminal IS the old server pane id (identity).
    let session = managed_session(
        "ws-1",
        "Workspace",
        term(old_pane),
        RuntimeEndpoint::Local,
        WorkspacePolicy::Persistent,
        Some(runtime_id),
    );
    let mut state = window_state(vec![session]);

    // Old pane resolves.
    assert_eq!(
        state.runtime_pane_target(&RuntimeEndpoint::Local, old_pane),
        Some(("ws-1".into(), old_pane.into())),
    );

    // Daemon restarts and sends a new authoritative single-pane tree.
    let mut snap = snapshot(runtime_id, vec![pane_snapshot(new_pane, "Shell", "/home")]);
    snap.tree = Some(pane_tree_leaf(uuid::Uuid::parse_str(new_pane).unwrap()));
    state.apply_managed_workspace_opened("ws-1", runtime_id, &snap);

    // Old pane should no longer resolve.
    assert!(state.runtime_pane_target(&RuntimeEndpoint::Local, old_pane).is_none());
    // New pane should resolve.
    assert_eq!(
        state.runtime_pane_target(&RuntimeEndpoint::Local, new_pane),
        Some(("ws-1".into(), new_pane.into())),
    );
}

/// Multiple workspaces on different endpoints maintain isolated indexes.
#[test]
fn multi_endpoint_isolation_through_full_lifecycle() {
    let local_runtime = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
    let remote_runtime = "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26";
    let local_pane = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
    let remote_pane = "0d88f17f-626d-40b8-a1d3-6a42af628ac9";
    let remote_endpoint = RuntimeEndpoint::remote("builder.example");

    let mut state = window_state(vec![
        managed_session(
            "ws-local",
            "Local",
            term("local-t1"),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            None,
        ),
        managed_session(
            "ws-remote",
            "Remote",
            term("remote-t1"),
            remote_endpoint.clone(),
            WorkspacePolicy::Persistent,
            None,
        ),
    ]);

    // Bind local pane.
    state.apply_managed_pane_created("ws-local", "local-t1", local_runtime, local_pane);
    // Bind remote pane.
    state.apply_managed_pane_created("ws-remote", "remote-t1", remote_runtime, remote_pane);

    // Each pane resolves only on its own endpoint (identity: uuid == pane id).
    assert_eq!(
        state.runtime_pane_target(&RuntimeEndpoint::Local, local_pane),
        Some(("ws-local".into(), local_pane.into())),
    );
    assert_eq!(
        state.runtime_pane_target(&remote_endpoint, remote_pane),
        Some(("ws-remote".into(), remote_pane.into())),
    );
    assert!(state.runtime_pane_target(&RuntimeEndpoint::Local, remote_pane).is_none());
    assert!(state.runtime_pane_target(&remote_endpoint, local_pane).is_none());
}
