//! Integration test: Pong is prioritized over Deltas in `client_writer`.
//!
//! Regression test for #557: when both `resp_rx` and `push_rx` have messages
//! ready, the biased select in `client_writer` must deliver control messages
//! (Pong) before data messages (Delta).

mod common;

use common::{TestClient, attach_rw, create_pane, create_session, start_test_server};
use rttx_proto::proto;

#[test]
fn pong_arrives_before_burst_deltas_are_fully_drained() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let tmp = tempfile::TempDir::new().unwrap();
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        let session_id =
            create_session(&mut client, "pong-priority", proto::RuntimePolicy::Persistent).await;
        let _snapshot = attach_rw(&mut client, &session_id).await;
        let pane_id = create_pane(&mut client, &session_id).await;

        // Generate a burst of PTY output to fill the push channel with Deltas.
        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::Input(proto::Input {
                    session_id: session_id.clone(),
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

        // Read messages until we see the Pong. Count how many Deltas
        // arrive before it — with biased select, the Pong should arrive
        // very quickly (within a few messages) rather than after all Deltas.
        let mut deltas_before_pong = 0u32;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut got_pong = false;

        while tokio::time::Instant::now() < deadline {
            match client.try_recv(std::time::Duration::from_secs(5)).await {
                Some(msg) => match msg.msg {
                    Some(proto::server_message::Msg::Pong(pong)) => {
                        assert_eq!(pong.nonce, 557);
                        got_pong = true;
                        break;
                    }
                    Some(proto::server_message::Msg::Delta(_)) => {
                        deltas_before_pong += 1;
                    }
                    _ => {}
                },
                None => break,
            }
        }

        assert!(got_pong, "pong must arrive during burst output");
        // The Pong should arrive promptly. With biased select it arrives
        // within the first few messages; without it, it could be delayed
        // behind hundreds or thousands of Deltas.
        assert!(
            deltas_before_pong < 50,
            "pong should arrive promptly, but {deltas_before_pong} deltas arrived first"
        );
    });
}
