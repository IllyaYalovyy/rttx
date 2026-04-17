//! Integration test: bounded push channel prevents slow clients from
//! blocking the server.

mod common;

use common::TestClient;
use common::{attach_rw, create_session, start_test_server};
use rttx_proto::proto;
use std::time::Duration;

/// A slow client that never reads from its connection should not prevent
/// the server from serving other clients. The server's bounded push
/// channel drops messages for the slow client while the fast client
/// continues to receive Deltas normally.
#[tokio::test]
async fn slow_client_does_not_block_server() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    // Fast client: creates session and pane, reads normally.
    let mut fast = TestClient::connect(&sock).await;
    fast.handshake().await;

    let session_id =
        create_session(&mut fast, "bounded-test", proto::RuntimePolicy::Persistent).await;
    let snapshot = attach_rw(&mut fast, &session_id).await;
    assert!(snapshot.panes.is_empty());

    // Create a pane that will produce output.
    fast.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
            session_id: session_id.clone(),
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
        })),
    })
    .await;
    let pane_id = match fast.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    // Slow client: attaches read-only but never reads after handshake.
    let mut slow = TestClient::connect(&sock).await;
    slow.handshake().await;
    slow.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
            session_id: session_id.clone(),
            attach_mode: proto::RuntimeAttachMode::ReadOnly as i32,
        })),
    })
    .await;
    // Read the snapshot so the handshake completes, then stop reading.
    let _snap = slow.recv_or_timeout().await;

    // Send input to generate output — the fast client should still
    // receive Deltas even if the slow client's channel fills up.
    fast.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Input(proto::Input {
            session_id: session_id.clone(),
            pane_id: pane_id.clone(),
            data: bytes::Bytes::from_static(b"echo bounded-channel-test\n"),
        })),
    })
    .await;

    // Fast client should receive Delta output within a reasonable time.
    let msgs = fast.drain(Duration::from_secs(5)).await;
    let has_delta =
        msgs.iter().any(|m| matches!(m.msg, Some(proto::server_message::Msg::Delta(_))));
    assert!(has_delta, "fast client should receive Deltas even with a slow client attached");
}
