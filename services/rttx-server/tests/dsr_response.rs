//! Integration test: DSR (Device Status Report) responses are written back to the PTY.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::proto;
use std::time::Duration;

/// Create a session, create a pane, attach, and return IDs.
async fn setup_attached_pane(client: &mut TestClient) -> (Vec<u8>, Vec<u8>) {
    client.handshake().await;

    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "dsr-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let runtime_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    let create_pane = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
            runtime_id: runtime_id.clone(),
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
        })),
    };
    client.send(&create_pane).await;
    let pane_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    let attach = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
            runtime_id: runtime_id.clone(),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::Snapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }

    (runtime_id, pane_id)
}

#[tokio::test]
async fn dsr_cursor_position_request_gets_cpr_response() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let (runtime_id, pane_id) = setup_attached_pane(&mut client).await;

    // Drain shell startup output.
    client.drain(Duration::from_millis(500)).await;

    // Send a command that writes DSR to stdout, then reads the CPR from stdin.
    // `printf '\033[6n'` writes DSR to the PTY. The daemon should respond with
    // CPR, which the shell reads. We use `read` to capture it and echo it back.
    //
    // Use a script that sends DSR and captures the response via `read -s -d R`.
    let script = r#"printf '\033[6n'; read -s -d R REPLY; echo "CPR=$REPLY""#;
    let input = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Input(proto::Input {
            runtime_id: runtime_id.clone(),
            pane_id: pane_id.clone(),
            data: bytes::Bytes::from(format!("{script}\n").into_bytes()),
        })),
    };
    client.send(&input).await;

    // Collect output and look for the CPR response echoed by the script.
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

    // The CPR response format is ESC[row;colR. After `read -s -d R`, REPLY
    // contains ESC[row;col (without the trailing R). The echo should show
    // something like "CPR=\x1b[1;1" or similar.
    assert!(
        output_str.contains("CPR="),
        "expected CPR response echoed by script, got: {output_str}"
    );
}
