//! Integration coverage for RFC-031 Step 4: the client renders its layout
//! purely from the server-authoritative pane tree (`WorkspaceSnapshot.tree`),
//! holding no pane identity or structure of its own.
//!
//! These tests exercise the public render entry points
//! (`workspace::layout_from_pane_tree` and
//! `workspace_state::render_layout_from_snapshot`) through the crate boundary,
//! the same way the attach path consumes a server snapshot.

use rttx::workspace::{LayoutNode, SplitOrientation, layout_from_pane_tree};
use rttx::workspace_state::render_layout_from_snapshot;
use rttx_proto::v3;
use rttx_proto::v3_tree::{pane_tree_leaf, pane_tree_split};
use uuid::Uuid;

fn leaf_uuids(layout: &LayoutNode) -> Vec<String> {
    layout.terminal_uuids()
}

fn snapshot_with_tree(tree: Option<v3::PaneTreeNode>) -> v3::WorkspaceSnapshot {
    v3::WorkspaceSnapshot {
        runtime_id: rttx_proto::uuid_to_bytes(Uuid::new_v4()),
        workspace_revision: 1,
        client_role: v3::WorkspaceClientRole::Writer as i32,
        panes: Vec::new(),
        tree,
        default_active_pane_id: Vec::new(),
    }
}

#[test]
fn client_renders_single_pane_workspace_from_server_tree() {
    let pane = Uuid::new_v4();
    let layout = layout_from_pane_tree(&pane_tree_leaf(pane)).expect("single leaf renders");

    // The render leaf is keyed by the durable server pane id — no client id.
    assert_eq!(leaf_uuids(&layout), vec![pane.to_string()]);
}

#[test]
fn client_renders_nested_split_structure_from_server_tree() {
    let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    // Horizontal( a , Vertical( b , c ) ).
    let tree = pane_tree_split(
        v3::PaneSplitAxis::Horizontal,
        0.6,
        pane_tree_leaf(a),
        pane_tree_split(v3::PaneSplitAxis::Vertical, 0.25, pane_tree_leaf(b), pane_tree_leaf(c)),
    );

    let layout = layout_from_pane_tree(&tree).expect("nested tree renders");
    let LayoutNode::Split { orientation, ratio, first, second } = layout else {
        panic!("expected a top-level split");
    };
    assert_eq!(orientation, SplitOrientation::Horizontal);
    assert!((ratio - 0.6).abs() < 1e-6);
    assert_eq!(leaf_uuids(&first), vec![a.to_string()]);

    let LayoutNode::Split { orientation, first: inner_a, second: inner_b, .. } = *second else {
        panic!("expected a nested split");
    };
    assert_eq!(orientation, SplitOrientation::Vertical);
    assert_eq!(leaf_uuids(&inner_a), vec![b.to_string()]);
    assert_eq!(leaf_uuids(&inner_b), vec![c.to_string()]);
}

#[test]
fn render_is_a_pure_function_of_the_server_tree() {
    // Two independent clients (two windows) attaching to the same workspace
    // build identical structure from the same server tree — shared structure,
    // no per-client identity (RFC-031 §3 viewport decoupling).
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let tree =
        pane_tree_split(v3::PaneSplitAxis::Vertical, 0.5, pane_tree_leaf(a), pane_tree_leaf(b));

    let window_one = layout_from_pane_tree(&tree).expect("renders");
    let window_two = layout_from_pane_tree(&tree).expect("renders");

    assert_eq!(window_one, window_two);
}

#[test]
fn snapshot_without_tree_yields_no_layout() {
    // An empty workspace carries no tree; the caller keeps its placeholder
    // back to its existing path rather than fabricating structure.
    assert!(render_layout_from_snapshot(&snapshot_with_tree(None)).is_none());
}

#[test]
fn snapshot_with_tree_renders_full_layout() {
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let snapshot = snapshot_with_tree(Some(pane_tree_split(
        v3::PaneSplitAxis::Horizontal,
        0.5,
        pane_tree_leaf(a),
        pane_tree_leaf(b),
    )));

    let layout = render_layout_from_snapshot(&snapshot).expect("snapshot with tree renders");
    let mut ids = leaf_uuids(&layout);
    ids.sort();
    let mut expected = vec![a.to_string(), b.to_string()];
    expected.sort();
    assert_eq!(ids, expected);
}

fn pane_snapshot(pane_id: Uuid, title: &str) -> v3::PaneSnapshot {
    v3::PaneSnapshot {
        pane_id: rttx_proto::uuid_to_bytes(pane_id),
        pane_output_seq: 0,
        title: title.to_string(),
        cwd: String::new(),
        cols: 80,
        rows: 24,
        exit_status: None,
        terminal_modes: None,
        scrollback_tail: bytes::Bytes::new(),
        total_scrollback_bytes: 0,
        scrollback_complete: true,
    }
}

/// The attach path (`reconcile_endpoint_event` → `WorkspaceOpened`) adopts the
/// server tree wholesale: the client discards its own stale layout, renders the
/// server pane ids, reports the discarded terminal for teardown, and — as a
/// pure view — never asks the daemon to create panes to "match".
#[test]
fn attach_adopts_server_tree_through_the_window_state_flow() {
    use rttx::daemon_bridge::EndpointEvent;
    use rttx::runtime::{WorkspacePolicy, WorkspaceRuntime};
    use rttx::workspace::{WindowState, WorkspaceState};

    let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";

    // The client holds a stale single-pane layout with a client-minted id.
    let mut session =
        WorkspaceState::new_managed_local("Home".into(), WorkspacePolicy::Persistent, None);
    session.uuid = "ws-1".into();
    session.layout = LayoutNode::Terminal {
        uuid: "stale-client-pane".into(),
        profile: None,
        cwd: None,
        custom_title: None,
    };
    session.runtime = WorkspaceRuntime::managed_local(WorkspacePolicy::Persistent);
    session.runtime.runtime_id = Some(runtime_id.into());

    let mut state = WindowState { workspaces: vec![session], ..WindowState::default() };

    // The daemon's authoritative tree: a horizontal split of two server panes.
    let pane_a = Uuid::new_v4();
    let pane_b = Uuid::new_v4();
    let mut snapshot = snapshot_with_tree(Some(pane_tree_split(
        v3::PaneSplitAxis::Horizontal,
        0.5,
        pane_tree_leaf(pane_a),
        pane_tree_leaf(pane_b),
    )));
    snapshot.runtime_id = rttx_proto::uuid_to_bytes(Uuid::parse_str(runtime_id).unwrap());
    snapshot.default_active_pane_id = rttx_proto::uuid_to_bytes(pane_b);
    snapshot.panes = vec![pane_snapshot(pane_a, "Shell"), pane_snapshot(pane_b, "Logs")];

    let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
        workspace_id: "ws-1".into(),
        runtime_id: runtime_id.into(),
        snapshot,
    });

    // The workspace is rebuilt with the server tree's structure.
    assert_eq!(transition.rebuilt_workspaces.len(), 1);
    let rebuilt = &transition.rebuilt_workspaces[0].session_state;
    let mut ids = rebuilt.layout.terminal_uuids();
    ids.sort();
    let mut expected = vec![pane_a.to_string(), pane_b.to_string()];
    expected.sort();
    assert_eq!(ids, expected, "the layout adopts the server tree's pane ids");

    // The stale client-owned pane is discarded and reported for widget teardown.
    assert!(
        transition.removed_layout_terminals.contains(&"stale-client-pane".to_string()),
        "the stale client layout terminal is torn down",
    );
    // A pure view never asks the daemon to create panes to match.
    assert!(transition.pane_create_requests.is_empty(), "pure view requests no pane creation");
    assert!(transition.skipped_runtime_panes.is_empty());
    // Pane content restores are addressed by the durable server pane id, and the
    // active pane follows the server's fallback focus.
    assert_eq!(transition.pane_snapshot_restores.len(), 2);
    assert_eq!(rebuilt.active_terminal_uuid.as_deref(), Some(pane_b.to_string().as_str()));
}

/// Reproduces the CI count-drift scenario at the state-machine level: adopt a
/// 2-pane split, close one pane, then reconnect to a 1-pane daemon tree. The
/// visible pane count must stay at one (no drift).
#[test]
fn close_after_adopt_then_reconnect_keeps_single_pane() {
    use rttx::daemon_bridge::EndpointEvent;
    use rttx::runtime::{WorkspacePolicy, WorkspaceRuntime};
    use rttx::workspace::{WindowState, WorkspaceState};

    let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
    let mut session =
        WorkspaceState::new_managed_local("Home".into(), WorkspacePolicy::Persistent, None);
    session.uuid = "ws-1".into();
    session.runtime = WorkspaceRuntime::managed_local(WorkspacePolicy::Persistent);
    session.runtime.runtime_id = Some(runtime_id.into());
    let mut state = WindowState { workspaces: vec![session], ..WindowState::default() };

    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let rt_bytes = rttx_proto::uuid_to_bytes(Uuid::parse_str(runtime_id).unwrap());

    // First reconnect after a split: adopt H(A, B) -> two panes.
    let mut snap = snapshot_with_tree(Some(pane_tree_split(
        v3::PaneSplitAxis::Horizontal,
        0.5,
        pane_tree_leaf(a),
        pane_tree_leaf(b),
    )));
    snap.runtime_id = rt_bytes.clone();
    snap.default_active_pane_id = rttx_proto::uuid_to_bytes(a);
    let _ = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
        workspace_id: "ws-1".into(),
        runtime_id: runtime_id.into(),
        snapshot: snap,
    });
    assert_eq!(
        state.workspaces[0].layout.terminal_uuids().len(),
        2,
        "two panes after adopting split"
    );

    // Close pane A.
    let _ = state.reconcile_endpoint_event(&EndpointEvent::PaneClosed {
        workspace_id: "ws-1".into(),
        layout_terminal_uuid: a.to_string(),
        runtime_id: runtime_id.into(),
        runtime_pane_id: a.to_string(),
    });
    assert_eq!(
        state.workspaces[0].layout.terminal_uuids(),
        vec![b.to_string()],
        "one pane after close"
    );

    // Second reconnect: the daemon tree is now just [B]. Adopting it must not
    // resurrect the closed pane.
    let mut snap2 = snapshot_with_tree(Some(pane_tree_leaf(b)));
    snap2.runtime_id = rt_bytes;
    snap2.default_active_pane_id = rttx_proto::uuid_to_bytes(b);
    let _ = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
        workspace_id: "ws-1".into(),
        runtime_id: runtime_id.into(),
        snapshot: snap2,
    });
    assert_eq!(
        state.workspaces[0].layout.terminal_uuids(),
        vec![b.to_string()],
        "still one pane after the second reconnect"
    );
}

/// A managed pane ack (the `CreatePane` bootstrap for a new workspace, or a
/// `SplitPane` reply) carries the server-minted pane id. The client re-keys its
/// optimistic, client-minted layout terminal onto that durable id so the
/// invariant `layout uuid == server pane id` holds everywhere — with no
/// binding table (RFC-031). The transition carries the re-key so the window
/// can rename its widget maps, and downstream connect/recover target the new id.
#[test]
fn pane_ack_rekeys_client_layout_terminal_to_server_pane_id() {
    use rttx::daemon_bridge::EndpointEvent;
    use rttx::runtime::{WorkspacePolicy, WorkspaceRuntime};
    use rttx::workspace::{WindowState, WorkspaceState};

    let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";

    let mut session =
        WorkspaceState::new_managed_local("Home".into(), WorkspacePolicy::Persistent, None);
    session.uuid = "ws-1".into();
    // The optimistic split (or new-workspace bootstrap) added a client-minted
    // layout terminal that is not yet a server pane id.
    session.layout = LayoutNode::Terminal {
        uuid: "client-minted-pane".into(),
        profile: None,
        cwd: None,
        custom_title: None,
    };
    session.active_terminal_uuid = Some("client-minted-pane".into());
    session.runtime = WorkspaceRuntime::managed_local(WorkspacePolicy::Persistent);
    session.runtime.runtime_id = Some(runtime_id.into());
    let mut state = WindowState { workspaces: vec![session], ..WindowState::default() };

    let server_pane = Uuid::new_v4();
    let transition = state.reconcile_endpoint_event(&EndpointEvent::PaneCreated {
        workspace_id: "ws-1".into(),
        layout_terminal_uuid: "client-minted-pane".into(),
        runtime_id: runtime_id.into(),
        runtime_pane_id: server_pane.to_string(),
    });

    // The layout terminal (and active pointer) are re-keyed to the server id.
    assert_eq!(state.workspaces[0].layout.terminal_uuids(), vec![server_pane.to_string()]);
    assert_eq!(
        state.workspaces[0].active_terminal_uuid.as_deref(),
        Some(server_pane.to_string().as_str())
    );

    // The transition reports the re-key (so the window renames its widget maps)
    // and targets the new identity uuid downstream.
    assert_eq!(transition.pane_rekeys.len(), 1);
    assert_eq!(transition.pane_rekeys[0].old_uuid, "client-minted-pane");
    assert_eq!(transition.pane_rekeys[0].new_uuid, server_pane.to_string());
    assert_eq!(transition.connected_layout_terminals, vec![server_pane.to_string()]);
}
