//! Integration test: PTY read loops yield under mutex contention (#827).
//!
//! Verifies that a client can complete a Ping round-trip within a tight
//! deadline while multiple panes produce continuous heavy output. Before
//! the adaptive throttle, N read loops would convoy on the mutex and
//! starve input handlers.

mod common;

use common::*;
use rttx_proto::v3;
use std::time::Duration;

#[tokio::test]
async fn ping_latency_stays_low_under_multi_pane_output() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;
    let sid = create_workspace(&mut client, "throttle-test", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &sid).await;

    // Create 4 panes producing heavy output simultaneously.
    let mut pane_ids = Vec::new();
    for _ in 0..4 {
        let pane_id = create_pane(&mut client, &sid).await;
        pane_ids.push(pane_id);
    }

    // Drain shell startup output.
    client.drain(Duration::from_millis(500)).await;

    // Start heavy output in all panes.
    for pane_id in &pane_ids {
        send_input(
            &mut client,
            &sid,
            pane_id,
            b"for i in $(seq 1 10000); do echo AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA$i; done\n",
        )
        .await;
    }

    // Let output build up.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send a Ping and verify it completes within a tight deadline.
    // The Ping fast-path bypasses the server mutex, but this test
    // validates that the overall system remains responsive — the
    // client_writer can drain its resp_rx because the push channel
    // is not monopolized by a single read loop.
    let ping = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::Ping(v3::Ping { nonce: 827 })),
    };
    client.send(&ping).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut got_pong = false;
    while tokio::time::Instant::now() < deadline {
        match client.try_recv(Duration::from_secs(2)).await {
            Some(msg) => {
                if let Some(v3::server_envelope::Payload::Pong(pong)) = msg.payload {
                    assert_eq!(pong.nonce, 827);
                    got_pong = true;
                    break;
                }
            }
            None => break,
        }
    }

    assert!(got_pong, "Ping must complete within 3s under multi-pane output load");
}
