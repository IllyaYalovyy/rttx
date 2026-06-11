//! Decisive daemon-level check for the close+restart pane-count drift: a pane
//! closed before a daemon restart must not resurrect when the workspace tree is
//! reconstructed from persisted state.

mod common;

use common::{
    TestClient, attach_rw, close_pane, create_pane, create_workspace, split_pane, start_test_server,
};
use rttx_proto::v3;
use std::time::Duration;

fn leaf_count(node: &v3::PaneTreeNode) -> usize {
    match node.node.as_ref() {
        Some(v3::pane_tree_node::Node::Leaf(_)) => 1,
        Some(v3::pane_tree_node::Node::Split(split)) => {
            split.first.as_ref().map_or(0, |n| leaf_count(n))
                + split.second.as_ref().map_or(0, |n| leaf_count(n))
        }
        None => 0,
    }
}

#[tokio::test]
async fn closed_pane_does_not_resurrect_after_daemon_restart() {
    let tmp = tempfile::TempDir::new().unwrap();
    let runtime_id;

    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;
        runtime_id =
            create_workspace(&mut client, "close-restart", v3::WorkspacePolicy::Persistent).await;
        attach_rw(&mut client, &runtime_id).await;

        let pane_a = create_pane(&mut client, &runtime_id).await;
        // Split A -> B, then close A; only B should remain in the tree.
        let _pane_b =
            split_pane(&mut client, &runtime_id, &pane_a, v3::PaneSplitAxis::Horizontal, 0.5)
                .await
                .new_pane_id;
        close_pane(&mut client, &runtime_id, &pane_a).await;

        // Let the 1s persistence ticker flush the collapsed (1-pane) tree before
        // the daemon goes away, mirroring a graceful restart.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(150)).await;
    }

    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;
        let snapshot = attach_rw(&mut client, &runtime_id).await;
        let tree = snapshot.tree.as_ref().expect("reconstructed workspace must carry a tree");
        assert_eq!(
            leaf_count(tree),
            1,
            "the closed pane must not resurrect after the daemon restart"
        );
    }
}
