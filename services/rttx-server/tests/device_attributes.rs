//! Integration test: DA1/DA2 device attribute responses are written back to the PTY.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::proto;
use std::time::Duration;

async fn setup_attached_pane(client: &mut TestClient) -> (Vec<u8>, Vec<u8>) {
    client.handshake().await;

    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
            name: "da-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let session_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::SessionCreated(sc)) => sc.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    let create_pane = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
            session_id: session_id.clone(),
            cwd: None,
            dark_background: None,
        })),
    };
    client.send(&create_pane).await;
    let pane_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    let attach = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
            session_id: session_id.clone(),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::Snapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }

    (session_id, pane_id)
}

#[tokio::test]
async fn da1_request_gets_device_attributes_response() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let (session_id, pane_id) = setup_attached_pane(&mut client).await;

    client.drain(Duration::from_millis(500)).await;

    // Send DA1 query and capture the response. The daemon should answer with
    // CSI ? 64;1;2;6;22 c. We use `read -s -d c` to capture up to the trailing 'c'.
    let script = r#"printf '\033[c'; read -s -d c REPLY; echo "DA1=$REPLY""#;
    let input = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Input(proto::Input {
            session_id: session_id.clone(),
            pane_id: pane_id.clone(),
            data: format!("{script}\n").into_bytes(),
        })),
    };
    client.send(&input).await;

    let msgs = client.drain(Duration::from_secs(5)).await;
    let output: Vec<u8> = msgs
        .iter()
        .filter_map(|m| match &m.msg {
            Some(proto::server_message::Msg::Delta(d)) => Some(d.data.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    let output_str = String::from_utf8_lossy(&output);

    assert!(
        output_str.contains("DA1="),
        "expected DA1 response echoed by script, got: {output_str}"
    );
}
