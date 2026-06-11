//! Integration coverage for the server-authoritative tree protocol
//! (RFC-031 §5, issue #1000).
//!
//! These tests drive the daemon over the real v3 wire protocol and assert the
//! behavior the protocol step must guarantee: attach returns the full
//! authoritative tree, structural mutations mint stable server-assigned pane
//! ids and emit correct deltas, and the multi-client PTY min-size policy keeps
//! a shared pane no larger than its smallest viewer.

mod common;

use common::{
    TestClient, attach_ro, attach_rw, create_pane, create_workspace, report_client_size,
    resize_split, resync, set_focus, split_pane, start_test_server,
};
use rttx_proto::v3;
use std::time::Duration;

/// Collect every leaf pane id in the proto tree, left to right.
fn leaf_ids(node: &v3::PaneTreeNode) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    collect(node, &mut out);
    out
}

fn collect(node: &v3::PaneTreeNode, out: &mut Vec<Vec<u8>>) {
    match node.node.as_ref() {
        Some(v3::pane_tree_node::Node::Leaf(leaf)) => out.push(leaf.pane_id.clone()),
        Some(v3::pane_tree_node::Node::Split(split)) => {
            if let Some(first) = split.first.as_ref() {
                collect(first, out);
            }
            if let Some(second) = split.second.as_ref() {
                collect(second, out);
            }
        }
        None => {}
    }
}

fn root_split(snapshot: &v3::WorkspaceSnapshot) -> v3::PaneTreeSplit {
    let node = snapshot.tree.as_ref().expect("snapshot must carry a tree");
    match node.node.as_ref() {
        Some(v3::pane_tree_node::Node::Split(split)) => (**split).clone(),
        other => panic!("expected a split at the tree root, got {other:?}"),
    }
}

#[tokio::test]
async fn attach_snapshot_carries_authoritative_tree() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;
    let runtime_id = create_workspace(&mut client, "tree", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &runtime_id).await;
    let pane_a = create_pane(&mut client, &runtime_id).await;
    let split =
        split_pane(&mut client, &runtime_id, &pane_a, v3::PaneSplitAxis::Horizontal, 0.5).await;
    let pane_b = split.new_pane_id.clone();

    let snapshot = resync(&mut client, &runtime_id).await;
    let tree = snapshot.tree.as_ref().expect("attach must return the full tree");
    let ids = leaf_ids(tree);
    assert_eq!(ids.len(), 2, "both panes must appear in the tree");
    assert!(ids.contains(&pane_a), "the original pane is in the tree");
    assert!(ids.contains(&pane_b), "the split pane is in the tree");
    // The split target stays the fallback focus.
    assert_eq!(snapshot.default_active_pane_id, pane_a);
    assert_eq!(snapshot.panes.len(), 2);
}

#[tokio::test]
async fn split_assigns_stable_server_pane_id_and_delta() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;
    let runtime_id = create_workspace(&mut client, "split", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &runtime_id).await;
    let pane_a = create_pane(&mut client, &runtime_id).await;

    let split =
        split_pane(&mut client, &runtime_id, &pane_a, v3::PaneSplitAxis::Vertical, 0.3).await;
    // The delta describes where the split landed; the new id is server-minted.
    assert_eq!(split.target_pane_id, pane_a);
    assert_eq!(split.axis, v3::PaneSplitAxis::Vertical as i32);
    assert!((split.ratio - 0.3).abs() < f32::EPSILON);
    assert_ne!(split.new_pane_id, pane_a, "server mints a fresh pane id");
    assert_eq!(split.new_pane_id.len(), 16, "pane id is a 16-byte uuid");
    assert!(split.workspace_revision > 0);

    // The server-assigned id is stable: it is exactly what the tree reports.
    let snapshot = resync(&mut client, &runtime_id).await;
    let ids = leaf_ids(snapshot.tree.as_ref().unwrap());
    assert!(ids.contains(&split.new_pane_id), "the minted id persists in the tree");
}

#[tokio::test]
async fn close_pane_collapses_the_tree() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;
    let runtime_id = create_workspace(&mut client, "close", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &runtime_id).await;
    let pane_a = create_pane(&mut client, &runtime_id).await;
    let split =
        split_pane(&mut client, &runtime_id, &pane_a, v3::PaneSplitAxis::Horizontal, 0.5).await;
    let pane_b = split.new_pane_id.clone();

    common::close_pane(&mut client, &runtime_id, &pane_b).await;

    let snapshot = resync(&mut client, &runtime_id).await;
    let tree = snapshot.tree.as_ref().expect("tree present after close");
    // The parent split collapses into the surviving leaf.
    assert!(
        matches!(tree.node.as_ref(), Some(v3::pane_tree_node::Node::Leaf(_))),
        "closing one of two panes collapses the split into a leaf",
    );
    assert_eq!(leaf_ids(tree), vec![pane_a]);
    assert_eq!(snapshot.panes.len(), 1);
}

#[tokio::test]
async fn resize_split_updates_logical_ratio() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;
    let runtime_id = create_workspace(&mut client, "resize", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &runtime_id).await;
    let pane_a = create_pane(&mut client, &runtime_id).await;
    split_pane(&mut client, &runtime_id, &pane_a, v3::PaneSplitAxis::Horizontal, 0.5).await;

    // The root split is addressed by the empty path.
    let resized = resize_split(&mut client, &runtime_id, &[], 0.2).await;
    assert!((resized.ratio - 0.2).abs() < f32::EPSILON);
    assert!(resized.workspace_revision > 0);

    let snapshot = resync(&mut client, &runtime_id).await;
    let split = root_split(&snapshot);
    assert!((split.ratio - 0.2).abs() < f32::EPSILON, "the durable ratio is updated");
}

#[tokio::test]
async fn resize_split_rejects_unaddressable_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;
    let runtime_id =
        create_workspace(&mut client, "resize-bad", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &runtime_id).await;
    let pane_a = create_pane(&mut client, &runtime_id).await;
    split_pane(&mut client, &runtime_id, &pane_a, v3::PaneSplitAxis::Horizontal, 0.5).await;

    // root.first is a leaf, not a split — the resize must be rejected.
    let reply = client
        .request(v3::client_envelope::Command::ResizeSplit(v3::ResizeSplit {
            runtime_id: runtime_id.clone(),
            path: vec![v3::PaneTreeSide::First as i32],
            ratio: 0.4,
        }))
        .await;
    assert!(
        matches!(reply.payload, Some(v3::server_envelope::Payload::Error(_))),
        "resizing a non-split node is an error, got {:?}",
        reply.payload,
    );
}

#[tokio::test]
async fn set_focus_updates_default_active() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;
    let runtime_id = create_workspace(&mut client, "focus", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &runtime_id).await;
    let pane_a = create_pane(&mut client, &runtime_id).await;
    let split =
        split_pane(&mut client, &runtime_id, &pane_a, v3::PaneSplitAxis::Horizontal, 0.5).await;
    let pane_b = split.new_pane_id.clone();

    let changed = set_focus(&mut client, &runtime_id, &pane_b).await;
    assert_eq!(changed.pane_id, pane_b);

    let snapshot = resync(&mut client, &runtime_id).await;
    assert_eq!(snapshot.default_active_pane_id, pane_b, "focus moved to the second pane");
}

#[tokio::test]
async fn multi_client_pty_size_is_the_minimum_across_clients() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    // Writer creates the workspace and a pane.
    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let runtime_id =
        create_workspace(&mut writer, "min-size", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut writer, &runtime_id).await;
    let pane = create_pane(&mut writer, &runtime_id).await;

    // A second client attaches read-only and shares the pane.
    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    attach_ro(&mut reader, &runtime_id).await;

    // The writer renders large; the reader renders small.
    report_client_size(&mut writer, &runtime_id, &[(pane.clone(), 120, 50)]).await;
    report_client_size(&mut reader, &runtime_id, &[(pane.clone(), 90, 30)]).await;

    // The PTY (reflected in the pane snapshot) tracks the per-axis minimum so
    // neither client sees truncated output (RFC-031 §4).
    let snapshot = resync(&mut writer, &runtime_id).await;
    let pane_snap =
        snapshot.panes.iter().find(|p| p.pane_id == pane).expect("pane present in snapshot");
    assert_eq!((pane_snap.cols, pane_snap.rows), (90, 30));
}

#[tokio::test]
async fn single_client_pty_tracks_that_client_size() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;
    let runtime_id =
        create_workspace(&mut client, "solo-size", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &runtime_id).await;
    let pane = create_pane(&mut client, &runtime_id).await;

    report_client_size(&mut client, &runtime_id, &[(pane.clone(), 100, 40)]).await;
    let _ = client.drain(Duration::from_millis(50)).await;

    let snapshot = resync(&mut client, &runtime_id).await;
    let pane_snap = snapshot.panes.iter().find(|p| p.pane_id == pane).expect("pane present");
    assert_eq!((pane_snap.cols, pane_snap.rows), (100, 40));
}

/// Mirrors the GUI repro that exposed the client-as-view bug: split a pane,
/// then split the *same* pane again on a different axis. The daemon tree must
/// stay correctly nested — `outer(inner(a, c), b)` — instead of flattening into
/// a row of three. (The client regression was sending `CreatePane` instead of
/// `SplitPane`, so the daemon never learned the structure; this guards the
/// daemon contract the pure-view client now depends on.)
#[tokio::test]
async fn nested_splits_keep_a_correctly_structured_tree() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;
    let runtime_id = create_workspace(&mut client, "nested", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &runtime_id).await;
    let pane_a = create_pane(&mut client, &runtime_id).await;

    // Split A horizontally -> B. Tree: H(A, B).
    let pane_b = split_pane(&mut client, &runtime_id, &pane_a, v3::PaneSplitAxis::Horizontal, 0.5)
        .await
        .new_pane_id;
    // Split A again, vertically -> C. Tree must nest: H(V(A, C), B).
    let pane_c = split_pane(&mut client, &runtime_id, &pane_a, v3::PaneSplitAxis::Vertical, 0.5)
        .await
        .new_pane_id;

    let snapshot = resync(&mut client, &runtime_id).await;
    assert_eq!(
        leaf_ids(snapshot.tree.as_ref().unwrap()).len(),
        3,
        "all three panes are present in the tree"
    );

    let root = root_split(&snapshot);
    assert_eq!(
        root.axis,
        v3::PaneSplitAxis::Horizontal as i32,
        "the outer split keeps the original horizontal axis, not a flattened row"
    );

    let first = root.first.expect("outer split has a first child");
    let second = root.second.expect("outer split has a second child");

    // One child is the lone leaf B; the other is the vertical split over {A, C}.
    let first_is_split = matches!(first.node, Some(v3::pane_tree_node::Node::Split(_)));
    let (nested, lone) = if first_is_split { (first, second) } else { (second, first) };

    assert_eq!(leaf_ids(&lone), vec![pane_b.clone()], "the un-split branch is pane B");

    let Some(v3::pane_tree_node::Node::Split(nested_split)) = nested.node.as_ref() else {
        panic!("one child of the outer split must itself be a split, not a flat row");
    };
    assert_eq!(
        nested_split.axis,
        v3::PaneSplitAxis::Vertical as i32,
        "the inner split keeps the vertical axis"
    );
    let mut nested_leaves = leaf_ids(&nested);
    nested_leaves.sort();
    let mut expected = vec![pane_a.clone(), pane_c.clone()];
    expected.sort();
    assert_eq!(nested_leaves, expected, "the inner split holds exactly A and C");
}
