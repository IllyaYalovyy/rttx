//! Integration test for #539: stale `HashMap` entries after workspace
//! reconciliation must be cleaned up via `removed_layout_terminals`.

use rttx::daemon_bridge::EndpointEvent;
use rttx::runtime::{WorkspacePolicy, WorkspaceRuntime};
use rttx::workspace::*;
use rttx::workspace_state::EndpointEventTransition;

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

fn managed_session(layout: LayoutNode, runtime_id: &str) -> WorkspaceState {
    let mut session =
        WorkspaceState::new_managed_local("Test".into(), WorkspacePolicy::Persistent, None);
    session.uuid = "ws-1".into();
    session.layout = layout;
    session.runtime = WorkspaceRuntime::managed_local(
        WorkspacePolicy::Persistent,
        &session.layout.terminal_uuids(),
    );
    session.runtime.runtime_id = Some(runtime_id.into());
    session
}

fn live_terminal_uuids(state: &WindowState) -> Vec<String> {
    state.workspaces.iter().flat_map(|s| s.layout.terminal_uuids()).collect()
}

/// Collect all terminal UUIDs that the transition would remove from the
/// `persistent_terminals` map (simulating what `apply_endpoint_event_transition` does).
fn stale_uuids_from_transition(transition: &EndpointEventTransition) -> Vec<String> {
    transition.removed_layout_terminals.clone()
}

/// After multiple reconnect cycles, the set of `removed_layout_terminals`
/// combined with the live layout UUIDs must account for every terminal
/// that was ever created — no UUID should be silently leaked.
#[test]
fn reconnect_cycles_never_leak_terminal_uuids() {
    let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
    let initial_pane = uuid::Uuid::new_v4().to_string();

    let session = managed_session(term("initial"), runtime_id);
    let mut state = WindowState { workspaces: vec![session], ..WindowState::default() };

    // Bind the initial terminal to a runtime pane.
    state.workspaces[0].runtime.bind_runtime_pane("initial", &initial_pane);

    let mut all_ever_created: std::collections::BTreeSet<String> =
        state.workspaces[0].layout.terminal_uuids().into_iter().collect();
    let mut all_removed: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for _cycle in 0..5 {
        let pane_a = uuid::Uuid::new_v4().to_string();
        let pane_b = uuid::Uuid::new_v4().to_string();

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.into(),
            snapshot: snapshot(
                runtime_id,
                vec![
                    pane_snapshot(&pane_a, "Shell", "/home"),
                    pane_snapshot(&pane_b, "Logs", "/var/log"),
                ],
            ),
        });

        // Track removed UUIDs.
        for uuid in stale_uuids_from_transition(&transition) {
            all_removed.insert(uuid);
        }

        // Track newly created UUIDs.
        let current_uuids = live_terminal_uuids(&state);
        for uuid in &current_uuids {
            all_ever_created.insert(uuid.clone());
        }
    }

    // Every UUID that was ever created must either be live or removed.
    let live_set: std::collections::BTreeSet<String> =
        live_terminal_uuids(&state).into_iter().collect();
    for uuid in &all_ever_created {
        assert!(
            live_set.contains(uuid) || all_removed.contains(uuid),
            "terminal UUID {uuid} was created but never removed and is not live — leaked"
        );
    }
}

/// Workspace reconciliation that produces a rebuild must not leave stale
/// terminal UUIDs unaccounted for in the transition.
#[test]
fn workspace_opened_transition_accounts_for_all_previous_terminals() {
    let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
    let pane_a = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
    let pane_b = "0d88f17f-626d-40b8-a1d3-6a42af628ac9";

    let mut session = managed_session(hsplit(term("left"), term("right")), runtime_id);
    session.runtime.bind_runtime_pane("left", pane_a);
    session.runtime.bind_runtime_pane("right", pane_b);
    let mut state = WindowState { workspaces: vec![session], ..WindowState::default() };

    let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
        workspace_id: "ws-1".into(),
        runtime_id: runtime_id.into(),
        snapshot: snapshot(
            runtime_id,
            vec![
                pane_snapshot(pane_a, "Shell", "/home"),
                pane_snapshot(pane_b, "Logs", "/var/log"),
            ],
        ),
    });

    let rebuilt = &transition.rebuilt_workspaces[0].session_state;
    let live_uuids: std::collections::BTreeSet<_> =
        rebuilt.layout.terminal_uuids().into_iter().collect();
    let removed: std::collections::BTreeSet<_> =
        transition.removed_layout_terminals.iter().cloned().collect();

    // Both "left" and "right" should be accounted for (live, not removed).
    assert!(live_uuids.contains("left"), "left should be live");
    assert!(live_uuids.contains("right"), "right should be live");
    assert!(removed.is_empty(), "no terminals should be removed when layout matches");
}

/// `PaneClosed` followed by `WorkspaceOpened` must not double-remove terminals.
#[test]
fn pane_closed_then_workspace_opened_does_not_double_remove() {
    let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
    let pane_a = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
    let pane_b = "0d88f17f-626d-40b8-a1d3-6a42af628ac9";

    let mut session = managed_session(hsplit(term("left"), term("right")), runtime_id);
    session.runtime.bind_runtime_pane("left", pane_a);
    session.runtime.bind_runtime_pane("right", pane_b);
    let mut state = WindowState { workspaces: vec![session], ..WindowState::default() };

    // Close "left" pane.
    let close_transition = state.reconcile_endpoint_event(&EndpointEvent::PaneClosed {
        workspace_id: "ws-1".into(),
        layout_terminal_uuid: "left".into(),
        runtime_id: runtime_id.into(),
        runtime_pane_id: pane_a.into(),
    });
    assert!(close_transition.removed_layout_terminals.contains(&"left".to_string()));

    // Reconnect with only pane_b.
    let open_transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
        workspace_id: "ws-1".into(),
        runtime_id: runtime_id.into(),
        snapshot: snapshot(runtime_id, vec![pane_snapshot(pane_b, "Logs", "/var/log")]),
    });

    // "left" was already removed by PaneClosed; it should not appear again.
    assert!(
        !open_transition.removed_layout_terminals.contains(&"left".to_string()),
        "already-removed terminal should not be double-removed"
    );
}
