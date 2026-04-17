//! Integration test: DSR/DA query sequences are stripped from client-bound output.
//!
//! Regression test for #582: applications that send DSR queries produce visible
//! `;1R` garbage because VTE generates duplicate CPR responses.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::proto;
use std::time::Duration;

async fn setup_attached_pane(client: &mut TestClient) -> (Vec<u8>, Vec<u8>) {
    client.handshake().await;

    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
            name: "strip-test".into(),
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

/// Verify that DSR query sequences do not appear in Delta messages sent to clients.
#[tokio::test]
async fn dsr_queries_stripped_from_delta_output() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let (session_id, pane_id) = setup_attached_pane(&mut client).await;

    client.drain(Duration::from_millis(500)).await;

    // Send a command that writes multiple DSR queries to stdout interleaved
    // with a known marker. The marker must appear in the Delta output, but
    // the raw ESC[6n sequences must not.
    let script = r"printf 'MARKER_A\033[6n\033[6nMARKER_B\033[c\033[>c'";
    let input = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Input(proto::Input {
            session_id: session_id.clone(),
            pane_id: pane_id.clone(),
            data: bytes::Bytes::from(format!("{script}\n").into_bytes()),
        })),
    };
    client.send(&input).await;

    let msgs = client.drain(Duration::from_secs(5)).await;
    let output: Vec<u8> = msgs
        .iter()
        .filter_map(|m| match &m.msg {
            Some(proto::server_message::Msg::Delta(d)) => Some(d.data.to_vec()),
            _ => None,
        })
        .flatten()
        .collect();

    // The raw ESC[6n (DSR cursor position query) must not appear in client output.
    assert!(
        !output.windows(4).any(|w| w == b"\x1b[6n"),
        "DSR cursor position query must be stripped from client output"
    );

    // The raw ESC[c (DA1 query) must not appear in client output.
    let has_da1_query = output.windows(3).enumerate().any(|(i, w)| {
        w == b"\x1b[c"
            && output.get(i.wrapping_sub(1)).is_none_or(|&b| b != b'?')
            && output.get(i + 3).is_none_or(|&b| b != b'?')
    });
    assert!(!has_da1_query, "DA1 query must be stripped from client output");

    // The markers should still be present (they are plain text).
    let output_str = String::from_utf8_lossy(&output);
    assert!(
        output_str.contains("MARKER_A") && output_str.contains("MARKER_B"),
        "markers must pass through: {output_str}"
    );
}
