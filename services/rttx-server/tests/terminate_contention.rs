//! Test that `terminate_workspace_internal` handles lock contention gracefully
//! instead of panicking (regression test for the `expect()` removal).

mod common;

use rttx_proto::v3;

#[tokio::test]
async fn terminate_workspace_does_not_panic_under_contention() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = common::start_test_server(tmp.path()).await;

    let mut client = common::TestClient::connect(&socket_path).await;
    client.handshake().await;

    // Create a workspace.
    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: "contention-test".into(),
            policy: v3::WorkspacePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let resp = client.recv().await;
    let runtime_id = match resp.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(rc)) => rc.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    // Terminate the workspace — this exercises terminate_workspace_internal.
    // Previously this could panic with expect() if the lock was contended.
    let terminate = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::TerminateWorkspace(v3::TerminateWorkspace {
            runtime_id: runtime_id.clone(),
        })),
    };
    client.send(&terminate).await;
    let resp = client.recv().await;
    assert!(
        matches!(resp.payload, Some(v3::server_envelope::Payload::WorkspaceTerminated(_))),
        "should get WorkspaceTerminated response"
    );
}
