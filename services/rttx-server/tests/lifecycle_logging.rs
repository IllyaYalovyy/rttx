//! Integration test: lifecycle events produce correct protocol responses
//! through the full server path (the same path that now emits log messages).

mod common;

use common::{
    attach_rw, close_pane, create_pane, create_workspace, detach_workspace, start_test_server,
    terminate_workspace,
};
use rttx_proto::v3;

#[tokio::test]
async fn full_lifecycle_produces_expected_responses() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;

    let mut client = common::TestClient::connect(&socket_path).await;
    client.handshake().await;

    // Create → attach → create pane → close pane → detach → terminate.
    let sid =
        create_workspace(&mut client, "lifecycle-log-test", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &sid).await;
    let pane_id = create_pane(&mut client, &sid).await;
    close_pane(&mut client, &sid, &pane_id).await;
    detach_workspace(&mut client, &sid).await;

    // Re-attach to terminate (need write access).
    attach_rw(&mut client, &sid).await;
    terminate_workspace(&mut client, &sid).await;

    // Session should be gone.
    let workspaces = common::list_workspaces(&mut client).await;
    assert!(workspaces.is_empty(), "session should be removed after terminate");
}

#[tokio::test]
async fn rename_workspace_through_server() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;

    let mut client = common::TestClient::connect(&socket_path).await;
    client.handshake().await;

    let sid = create_workspace(&mut client, "before-rename", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &sid).await;

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::RenameWorkspace(v3::RenameWorkspace {
                runtime_id: sid.clone(),
                name: "after-rename".into(),
            })),
        })
        .await;

    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceRenamed(sr)) => {
                assert_eq!(sr.name, "after-rename");
                break;
            }
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected WorkspaceRenamed, got {other:?}"),
        }
    }

    let workspaces = common::list_workspaces(&mut client).await;
    assert_eq!(workspaces[0].name, "after-rename");
}
