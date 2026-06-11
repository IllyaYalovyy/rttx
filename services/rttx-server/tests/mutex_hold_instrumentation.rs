//! Regression test: server mutex is not held during PTY I/O (#546).
//!
//! Verifies that a second client can complete a Ping round-trip while
//! a pane is producing continuous output. If the mutex were held during
//! PTY writes, the Ping would stall until the output burst finished.

mod common;

use common::*;
use rttx_proto::v3;
use std::time::Duration;

#[tokio::test]
async fn ping_succeeds_during_pty_output_burst() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    // Client A: create session, attach, create pane, start output burst.
    let mut client_a = TestClient::connect(&sock).await;
    client_a.handshake().await;
    let sid = create_workspace(&mut client_a, "mutex-test", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client_a, &sid).await;
    let pane_id = create_pane(&mut client_a, &sid).await;

    // Drain shell startup.
    client_a.drain(Duration::from_millis(500)).await;

    // Start a long-running output burst (seq prints many lines).
    send_input(&mut client_a, &sid, &pane_id, b"seq 1 5000\n").await;

    // Client B: connect and verify Ping completes promptly.
    let mut client_b = TestClient::connect(&sock).await;
    client_b.handshake().await;

    let ping = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::Ping(v3::Ping { nonce: 546 })),
    };
    client_b.send(&ping).await;

    // The Ping fast-path bypasses the server mutex entirely, so this
    // should complete well within 2 seconds even under heavy PTY output.
    let resp = client_b
        .try_recv(Duration::from_secs(2))
        .await
        .expect("Ping timed out — server mutex may be blocking PTY read loop");

    match resp.payload {
        Some(v3::server_envelope::Payload::Pong(p)) => assert_eq!(p.nonce, 546),
        other => panic!("expected Pong, got {other:?}"),
    }
}
