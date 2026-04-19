mod common;

use common::{TestClient, start_test_server};
use rttx_proto::proto;

/// Helper: create a pane with explicit cols/rows and return its `pane_id`.
async fn create_pane_with_size(
    client: &mut TestClient,
    runtime_id: &[u8],
    cols: u32,
    rows: u32,
) -> Vec<u8> {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                runtime_id: runtime_id.to_vec(),
                cwd: None,
                dark_background: None,
                cols,
                rows,
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::PaneCreated(pc)) => return pc.pane_id,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected PaneCreated, got {other:?}"),
        }
    }
}

/// Helper: create a session and return its id.
async fn create_runtime(client: &mut TestClient, name: &str) -> Vec<u8> {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
                name: name.into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    }
}

/// Helper: attach and return the snapshot.
async fn attach_and_snapshot(client: &mut TestClient, runtime_id: &[u8]) -> proto::Snapshot {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
                runtime_id: runtime_id.to_vec(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::Snapshot(s)) => return s,
            Some(proto::server_message::Msg::Delta(_)) => {}
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

    let runtime_id = create_runtime(&mut client, "size-test").await;
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

    let runtime_id = create_runtime(&mut client, "zero-size").await;
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
