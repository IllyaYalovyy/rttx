//! Regression test: PTY read loop broadcasts Deltas after releasing the
//! server mutex (#558).
//!
//! Verifies that two clients attached to the same session both receive
//! Delta messages from a pane. This exercises the `send_to_collected`
//! path where sender handles are cloned under the lock and used after
//! releasing it.

mod common;

use common::*;
use rttx_proto::proto;
use std::time::Duration;

#[tokio::test]
async fn both_clients_receive_deltas_after_lock_free_broadcast() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    // Client A creates and attaches to a session.
    let mut client_a = TestClient::connect(&sock).await;
    client_a.handshake().await;
    let sid =
        create_runtime(&mut client_a, "broadcast-test", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut client_a, &sid).await;
    let pane_id = create_pane(&mut client_a, &sid).await;

    // Client B attaches read-only to the same session.
    let mut client_b = TestClient::connect(&sock).await;
    client_b.handshake().await;
    attach_ro(&mut client_b, &sid).await;

    // Send input that produces output.
    send_input(&mut client_a, &sid, &pane_id, b"echo LOCKFREE\n").await;

    // Both clients should receive at least one Delta containing the output.
    let has_delta = |msgs: &[proto::ServerMessage]| {
        msgs.iter().any(|m| matches!(&m.msg, Some(proto::server_message::Msg::Delta(_))))
    };

    let msgs_a = client_a.drain(Duration::from_secs(3)).await;
    let msgs_b = client_b.drain(Duration::from_secs(3)).await;

    assert!(has_delta(&msgs_a), "client A should receive Deltas");
    assert!(has_delta(&msgs_b), "client B should receive Deltas");
}
