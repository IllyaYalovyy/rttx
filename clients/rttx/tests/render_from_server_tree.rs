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
    // A pre-tree daemon or empty workspace carries no tree; the caller falls
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
