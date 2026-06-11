//! Decisive daemon-level check for the close+restart pane-count drift: a pane
//! closed before a *graceful* daemon shutdown must not resurrect when the
//! workspace tree is reconstructed from persisted state.
//!
//! Uses the real graceful shutdown path (the `Shutdown` command -> the run
//! loop's `persist_and_cleanup`), not a periodic-flush wait, so it mirrors what
//! `rttx-server stop` does in the GUI repro.

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

/// Send the graceful `Shutdown` command and wait for `run()` to finish, which
/// guarantees `persist_and_cleanup` has flushed state to disk.
async fn graceful_shutdown(
    client: &mut TestClient,
    handle: tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::Shutdown(v3::Shutdown {})),
        })
        .await;
    let _ = tokio::time::timeout(Duration::from_secs(10), handle).await;
}

#[tokio::test]
async fn closed_pane_does_not_resurrect_after_graceful_restart() {
    let tmp = tempfile::TempDir::new().unwrap();
    let runtime_id;
    let pane_a;

    // Phase 1: create a 2-pane workspace, then gracefully shut down.
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;
        runtime_id =
            create_workspace(&mut client, "close-restart", v3::WorkspacePolicy::Persistent).await;
        attach_rw(&mut client, &runtime_id).await;
        pane_a = create_pane(&mut client, &runtime_id).await;
        let _pane_b =
            split_pane(&mut client, &runtime_id, &pane_a, v3::PaneSplitAxis::Horizontal, 0.5)
                .await
                .new_pane_id;
        graceful_shutdown(&mut client, handle).await;
    }

    // Phase 2: reattach to the reconstructed workspace, close pane A, shut down.
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;
        let snapshot = attach_rw(&mut client, &runtime_id).await;
        assert_eq!(leaf_count(snapshot.tree.as_ref().unwrap()), 2, "two panes restored");

        close_pane(&mut client, &runtime_id, &pane_a).await;
        graceful_shutdown(&mut client, handle).await;
    }

    // Phase 3: reattach — the closed pane must not resurrect.
    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;
        let snapshot = attach_rw(&mut client, &runtime_id).await;
        let tree = snapshot.tree.as_ref().expect("reconstructed workspace must carry a tree");
        assert_eq!(
            leaf_count(tree),
            1,
            "a pane closed before a graceful restart must not resurrect"
        );
    }
}
