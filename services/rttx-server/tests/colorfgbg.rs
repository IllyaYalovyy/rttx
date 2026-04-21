mod common;

use common::{TestClient, start_test_server};
use rttx_proto::proto;
use std::time::Duration;

/// Helper: create a pane with explicit `dark_background`, return `pane_id`.
async fn create_pane_with_appearance(
    client: &mut TestClient,
    runtime_id: &[u8],
    dark_background: Option<bool>,
) -> Vec<u8> {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                runtime_id: runtime_id.to_vec(),
                cwd: None,
                dark_background,
                cols: 0,
                rows: 0,
                no_persist: None,
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

/// Read PTY output until `needle` is found or timeout.
async fn read_until(client: &mut TestClient, needle: &str, timeout: Duration) -> String {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut output = String::new();
    while tokio::time::Instant::now() < deadline {
        if let Some(msg) = client.try_recv(Duration::from_millis(200)).await
            && let Some(proto::server_message::Msg::Delta(delta)) = msg.msg
        {
            output.push_str(&String::from_utf8_lossy(&delta.data));
            if output.contains(needle) {
                return output;
            }
        }
    }
    output
}

/// Send input and drain snapshot first.
async fn attach_and_send(client: &mut TestClient, runtime_id: &[u8], pane_id: &[u8], input: &[u8]) {
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
            Some(proto::server_message::Msg::Snapshot(_)) => break,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }
    }
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Input(proto::Input {
                runtime_id: runtime_id.to_vec(),
                pane_id: pane_id.to_vec(),
                data: bytes::Bytes::copy_from_slice(input),
            })),
        })
        .await;
}

/// Dark background pane must have COLORFGBG=15;0.
#[tokio::test]
async fn create_pane_dark_sets_colorfgbg() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let runtime_id =
        common::create_runtime(&mut client, "dark-test", proto::RuntimePolicy::Persistent).await;

    let pane_id = create_pane_with_appearance(&mut client, &runtime_id, Some(true)).await;
    attach_and_send(&mut client, &runtime_id, &pane_id, b"echo $COLORFGBG\n").await;

    let output = read_until(&mut client, "15;0", Duration::from_secs(5)).await;
    assert!(output.contains("15;0"), "dark pane must have COLORFGBG=15;0, got: {output}");
}

/// Light background pane must have COLORFGBG=0;15.
#[tokio::test]
async fn create_pane_light_sets_colorfgbg() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let runtime_id =
        common::create_runtime(&mut client, "light-test", proto::RuntimePolicy::Persistent).await;

    let pane_id = create_pane_with_appearance(&mut client, &runtime_id, Some(false)).await;
    attach_and_send(&mut client, &runtime_id, &pane_id, b"echo $COLORFGBG\n").await;

    let output = read_until(&mut client, "0;15", Duration::from_secs(5)).await;
    assert!(output.contains("0;15"), "light pane must have COLORFGBG=0;15, got: {output}");
}

/// Omitting `dark_background` (None) defaults to dark (15;0).
#[tokio::test]
async fn create_pane_default_assumes_dark() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let runtime_id =
        common::create_runtime(&mut client, "default-test", proto::RuntimePolicy::Persistent).await;

    let pane_id = create_pane_with_appearance(&mut client, &runtime_id, None).await;
    attach_and_send(&mut client, &runtime_id, &pane_id, b"echo $COLORFGBG\n").await;

    let output = read_until(&mut client, "15;0", Duration::from_secs(5)).await;
    assert!(
        output.contains("15;0"),
        "pane without dark_background must default to dark (15;0), got: {output}"
    );
}
