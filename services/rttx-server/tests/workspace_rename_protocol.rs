//! End-to-end regression for the RFC-031 Step 6 `Runtime`→`Workspace` rename.
//!
//! The rename touched the v3 lifecycle commands (`CreateWorkspace`,
//! `ListWorkspaces`) and the `WorkspacePolicy` enum on both ends of the wire.
//! This test drives the renamed path through a live daemon — create two
//! workspaces with different policies, then list them — to prove the rename
//! preserved create/list semantics and the per-workspace policy round-trip.

mod common;

use common::*;
use rttx_proto::v3;

#[tokio::test]
async fn create_then_list_workspaces_preserves_name_and_policy() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let persistent_id =
        common::create_workspace(&mut client, "ws-persistent", v3::WorkspacePolicy::Persistent)
            .await;
    let ephemeral_id =
        common::create_workspace(&mut client, "ws-ephemeral", v3::WorkspacePolicy::Ephemeral).await;

    // Server-assigned ids are 16-byte UUIDs and distinct per workspace.
    assert_eq!(persistent_id.len(), 16);
    assert_eq!(ephemeral_id.len(), 16);
    assert_ne!(persistent_id, ephemeral_id);

    let workspaces = common::list_workspaces(&mut client).await;

    let persistent = workspaces
        .iter()
        .find(|w| w.name == "ws-persistent")
        .expect("persistent workspace must appear in ListWorkspaces");
    assert_eq!(persistent.policy, v3::WorkspacePolicy::Persistent as i32);

    let ephemeral = workspaces
        .iter()
        .find(|w| w.name == "ws-ephemeral")
        .expect("ephemeral workspace must appear in ListWorkspaces");
    assert_eq!(ephemeral.policy, v3::WorkspacePolicy::Ephemeral as i32);
}
