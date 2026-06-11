mod common;

use common::{TestClient, start_test_server};
use rttx_proto::v3;

/// Helper: create a pane with explicit cols/rows and return its `pane_id`.
async fn create_pane_with_size(
    client: &mut TestClient,
    runtime_id: &[u8],
    cols: u32,
    rows: u32,
) -> Vec<u8> {
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
                runtime_id: runtime_id.to_vec(),
                cwd: None,
                dark_background: None,
                cols,
                rows,
                no_persist: None,
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::PaneCreated(pc)) => return pc.pane_id,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected PaneCreated, got {other:?}"),
        }
    }
}

/// Helper: create a session and return its id.
async fn create_workspace(client: &mut TestClient, name: &str) -> Vec<u8> {
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
                name: name.into(),
                policy: v3::WorkspacePolicy::Persistent as i32,
            })),
        })
        .await;
    match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(sc)) => sc.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    }
}

/// Helper: attach and return the snapshot.
async fn attach_and_snapshot(client: &mut TestClient, runtime_id: &[u8]) -> v3::WorkspaceSnapshot {
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.to_vec(),
                attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceSnapshot(s)) => return s,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }
    }
}

/// `CreatePane` with explicit cols/rows creates PTY at that size. #636.
#[tokio::test]
async fn create_pane_uses_requested_dimensions() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let runtime_id = create_workspace(&mut client, "size-test").await;
    let pane_id = create_pane_with_size(&mut client, &runtime_id, 132, 43).await;
    let snapshot = attach_and_snapshot(&mut client, &runtime_id).await;

    let pane = snapshot
        .panes
        .iter()
        .find(|p| p.pane_id == pane_id)
        .expect("pane should appear in snapshot");
    assert_eq!(pane.cols, 132, "pane cols should match requested size");
    assert_eq!(pane.rows, 43, "pane rows should match requested size");
}

/// `CreatePane` with zero cols/rows falls back to 80×24. #636.
#[tokio::test]
async fn create_pane_zero_size_falls_back_to_default() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let runtime_id = create_workspace(&mut client, "zero-size").await;
    let pane_id = create_pane_with_size(&mut client, &runtime_id, 0, 0).await;
    let snapshot = attach_and_snapshot(&mut client, &runtime_id).await;

    let pane = snapshot
        .panes
        .iter()
        .find(|p| p.pane_id == pane_id)
        .expect("pane should appear in snapshot");
    assert_eq!(pane.cols, 80, "zero cols should fall back to 80");
    assert_eq!(pane.rows, 24, "zero rows should fall back to 24");
}
