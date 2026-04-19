//! Integration test: Pong is prioritized over Deltas in `client_writer`.
//!
//! Regression test for #557: when both `resp_rx` and `push_rx` have messages
//! ready, the biased select in `client_writer` must deliver control messages
//! (Pong) before data messages (Delta).

mod common;

use common::{TestClient, attach_rw, create_pane, create_runtime, start_test_server};
use rttx_proto::proto;

#[test]
fn pong_arrives_promptly_during_burst_output() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let tmp = tempfile::TempDir::new().unwrap();
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        let runtime_id =
            create_runtime(&mut client, "pong-priority", proto::RuntimePolicy::Persistent).await;
        let _snapshot = attach_rw(&mut client, &runtime_id).await;
        let pane_id = create_pane(&mut client, &runtime_id).await;

        // Generate a burst of PTY output to fill the push channel with Deltas.
        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::Input(proto::Input {
                    runtime_id: runtime_id.clone(),
                    pane_id,
                    data: bytes::Bytes::from_static(
                        b"for i in $(seq 1 1000); do echo AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA$i; done\n",
                    ),
                })),
            })
            .await;

        // Let output accumulate in the push channel.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Send a ping while Deltas are queued.
        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::Ping(proto::Ping { nonce: 557 })),
            })
            .await;

        // The Pong must arrive within a tight deadline even though the
        // push channel is saturated with Deltas.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut got_pong = false;

        while tokio::time::Instant::now() < deadline {
            match client.try_recv(std::time::Duration::from_secs(3)).await {
                Some(msg) => {
                    if let Some(proto::server_message::Msg::Pong(pong)) = msg.msg {
                        assert_eq!(pong.nonce, 557);
                        got_pong = true;
                        break;
                    }
                }
                None => break,
            }
        }

        assert!(got_pong, "pong must arrive during burst output");
    });
}
