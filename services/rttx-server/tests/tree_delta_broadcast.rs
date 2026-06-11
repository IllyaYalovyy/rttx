//! Integration test: workspace-tree mutations by one client are broadcast as
//! deltas to *other* attached clients (RFC-031 §5).
//!
//! This is the server-side foundation the client-as-view depends on (step 1a):
//! when one client splits or closes a pane, every other attached client must
//! receive the corresponding tree delta (`PaneSplit` / `PaneClosed`) as a push
//! event so it can update its view without re-attaching.

mod common;

use common::{
    attach_ro, attach_rw, close_pane, create_pane, create_workspace, split_pane,
    start_test_server, TestClient,
};
use rttx_proto::v3;
use std::time::Duration;

#[tokio::test]
async fn split_and_close_are_broadcast_to_other_attached_clients() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    // Writer client: owns the workspace and one pane.
    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let runtime_id =
        create_workspace(&mut writer, "tree-fanout", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut writer, &runtime_id).await;
    let pane_a = create_pane(&mut writer, &runtime_id).await;

    // Observer client: attaches read-only to the same workspace.
    let mut observer = TestClient::connect(&sock).await;
    observer.handshake().await;
    attach_ro(&mut observer, &runtime_id).await;

    // Drain shell startup output so it can't mask the deltas under test.
    let _ = writer.drain(Duration::from_millis(300)).await;
    let _ = observer.drain(Duration::from_millis(300)).await;

    // The writer splits its pane. The observer must receive the PaneSplit delta.
    let split =
        split_pane(&mut writer, &runtime_id, &pane_a, v3::PaneSplitAxis::Vertical, 0.4).await;
    let env = observer
        .recv_matching(|p| matches!(p, v3::server_envelope::Payload::PaneSplit(_)))
        .await;
    let Some(v3::server_envelope::Payload::PaneSplit(observed)) = env.payload else {
        unreachable!("recv_matching guaranteed PaneSplit");
    };
    assert_eq!(observed.new_pane_id, split.new_pane_id, "observer sees the same new pane id");
    assert_eq!(observed.target_pane_id, pane_a, "split targeted pane_a");
    assert_eq!(observed.axis, v3::PaneSplitAxis::Vertical as i32);
    assert_eq!(env.request_id, 0, "a broadcast push carries request_id 0");

    // The writer closes the new pane. The observer must receive a PaneClosed delta.
    close_pane(&mut writer, &runtime_id, &split.new_pane_id).await;
    let env = observer
        .recv_matching(|p| matches!(p, v3::server_envelope::Payload::PaneClosed(_)))
        .await;
    let Some(v3::server_envelope::Payload::PaneClosed(closed)) = env.payload else {
        unreachable!("recv_matching guaranteed PaneClosed");
    };
    assert_eq!(closed.pane_id, split.new_pane_id, "observer sees the closed pane id");
    assert_eq!(env.request_id, 0, "a broadcast push carries request_id 0");
}
