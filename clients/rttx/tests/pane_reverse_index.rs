//! Integration tests for the pane reverse index.
//!
//! Verifies that `runtime_pane_target()` returns correct results through
//! full workspace lifecycle scenarios: create, bind, reconcile, close.

use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy, WorkspaceRuntime};
use rttx::workspace::*;
use std::collections::{BTreeMap, BTreeSet};

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
    let mut session = WorkspaceState {
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
            pane_bindings: BTreeMap::default(),
            pending_layout_panes: BTreeSet::default(),
        },
        color: WorkspaceColor::default(),
        zoomed_terminal_uuid: None,
        user_renamed: false,
    };
    session.runtime.ensure_placeholder_bindings(&session.layout.terminal_uuids());
    session
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
) -> rttx_proto::v3::RuntimeSnapshot {
    rttx_proto::v3::RuntimeSnapshot {
        runtime_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(runtime_id).unwrap()),
        panes,
        runtime_revision: 1,
        client_role: rttx_proto::v3::RuntimeClientRole::Writer as i32,
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

    // Before binding: no lookups should resolve.
    assert!(state.runtime_pane_target(&RuntimeEndpoint::Local, pane_a).is_none());

    // Bind panes via apply_managed_pane_created.
    state.apply_managed_pane_created("ws-1", "left", runtime_id, pane_a);
    state.apply_managed_pane_created("ws-1", "right", runtime_id, pane_b);

    // Both panes should resolve.
    assert_eq!(
        state.runtime_pane_target(&RuntimeEndpoint::Local, pane_a),
        Some(("ws-1".into(), "left".into())),
    );
    assert_eq!(
        state.runtime_pane_target(&RuntimeEndpoint::Local, pane_b),
        Some(("ws-1".into(), "right".into())),
    );

    // Close one pane.
    state.apply_managed_pane_closed("ws-1", "left");

    // Closed pane should no longer resolve.
    assert!(state.runtime_pane_target(&RuntimeEndpoint::Local, pane_a).is_none());
    // Remaining pane should still resolve.
    assert_eq!(
        state.runtime_pane_target(&RuntimeEndpoint::Local, pane_b),
        Some(("ws-1".into(), "right".into())),
    );
}

/// Reconciliation through `workspace_opened` replaces stale bindings and
/// the index reflects the new state.
#[test]
fn reconciliation_updates_index_after_daemon_restart() {
    let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
    let old_pane = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
    let new_pane = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";

    let mut session = managed_session(
        "ws-1",
        "Workspace",
        term("t1"),
        RuntimeEndpoint::Local,
        WorkspacePolicy::Persistent,
        Some(runtime_id),
    );
    session.runtime.bind_runtime_pane("t1", old_pane);
    let mut state = window_state(vec![session]);

    // Old pane resolves.
    assert_eq!(
        state.runtime_pane_target(&RuntimeEndpoint::Local, old_pane),
        Some(("ws-1".into(), "t1".into())),
    );

    // Daemon restarts with a new pane ID.
    state.apply_managed_workspace_opened(
        "ws-1",
        runtime_id,
        &snapshot(runtime_id, vec![pane_snapshot(new_pane, "Shell", "/home")]),
    );

    // Old pane should no longer resolve.
    assert!(state.runtime_pane_target(&RuntimeEndpoint::Local, old_pane).is_none());
    // New pane should resolve.
    assert_eq!(
        state.runtime_pane_target(&RuntimeEndpoint::Local, new_pane),
        Some(("ws-1".into(), "t1".into())),
    );
}

/// Multiple workspaces on different endpoints maintain isolated indexes.
#[test]
fn multi_endpoint_isolation_through_full_lifecycle() {
    let local_runtime = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
    let remote_runtime = "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26";
    let local_pane = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
    let remote_pane = "0d88f17f-626d-40b8-a1d3-6a42af628ac9";
    let remote_endpoint = RuntimeEndpoint::Remote { host: "builder.example".into() };

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

    // Each pane resolves only on its own endpoint.
    assert_eq!(
        state.runtime_pane_target(&RuntimeEndpoint::Local, local_pane),
        Some(("ws-local".into(), "local-t1".into())),
    );
    assert_eq!(
        state.runtime_pane_target(&remote_endpoint, remote_pane),
        Some(("ws-remote".into(), "remote-t1".into())),
    );
    assert!(state.runtime_pane_target(&RuntimeEndpoint::Local, remote_pane).is_none());
    assert!(state.runtime_pane_target(&remote_endpoint, local_pane).is_none());
}
