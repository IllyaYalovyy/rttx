mod common;

use common::{TestClient, start_test_server};
use rttx_proto::v3;
use std::time::Duration;

/// Helper: create a pane with explicit `dark_background`, return `pane_id`.
async fn create_pane_with_appearance(
    client: &mut TestClient,
    runtime_id: &[u8],
    dark_background: Option<bool>,
) -> Vec<u8> {
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
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
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::PaneCreated(pc)) => return pc.pane_id,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
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
            && let Some(v3::server_envelope::Payload::OutputDelta(delta)) = msg.payload
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
            Some(v3::server_envelope::Payload::WorkspaceSnapshot(_)) => break,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }
    }
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
                runtime_id: runtime_id.to_vec(),
                pane_id: pane_id.to_vec(),
                kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                    data: bytes::Bytes::copy_from_slice(input),
                })),
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
        common::create_workspace(&mut client, "dark-test", v3::WorkspacePolicy::Persistent).await;

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
        common::create_workspace(&mut client, "light-test", v3::WorkspacePolicy::Persistent).await;

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
        common::create_workspace(&mut client, "default-test", v3::WorkspacePolicy::Persistent).await;

    let pane_id = create_pane_with_appearance(&mut client, &runtime_id, None).await;
    attach_and_send(&mut client, &runtime_id, &pane_id, b"echo $COLORFGBG\n").await;

    let output = read_until(&mut client, "15;0", Duration::from_secs(5)).await;
    assert!(
        output.contains("15;0"),
        "pane without dark_background must default to dark (15;0), got: {output}"
    );
}
