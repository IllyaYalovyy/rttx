//! Integration tests for daemon heartbeat messages.

mod common;

use common::{TestClient, attach_rw, create_session, start_test_server};
use rttx_proto::proto;

#[test]
fn ping_receives_matching_pong() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let tmp = tempfile::TempDir::new().unwrap();
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::Ping(proto::Ping { nonce: 42 })),
            })
            .await;

        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::Pong(pong)) => assert_eq!(pong.nonce, 42),
            other => panic!("expected Pong, got {other:?}"),
        }
    });
}

#[test]
fn ping_roundtrip_still_works_for_attached_clients() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let tmp = tempfile::TempDir::new().unwrap();
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        let session_id =
            create_session(&mut client, "heartbeat-attach", proto::RuntimePolicy::Persistent).await;
        let _snapshot = attach_rw(&mut client, &session_id).await;

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::Ping(proto::Ping { nonce: 7 })),
            })
            .await;

        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::Pong(pong)) => assert_eq!(pong.nonce, 7),
            other => panic!("expected Pong, got {other:?}"),
        }
    });
}
