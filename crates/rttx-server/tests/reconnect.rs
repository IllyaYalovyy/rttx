//! Integration tests for client reconnection.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::proto;

#[tokio::test]
async fn reconnect_after_disconnect() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    // First client: create session, then disconnect.
    let session_id = {
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        let create = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: "reconnect-test".into(),
            })),
        };
        client.send(&create).await;
        let resp = client.recv().await;
        match resp.msg {
            Some(proto::server_message::Msg::SessionCreated(sc)) => sc.session_id,
            other => panic!("expected SessionCreated, got {other:?}"),
        }
        // client drops here — simulates GUI crash
    };

    // Second client: reconnect and verify session still exists.
    let mut client2 = TestClient::connect(&sock).await;
    client2.handshake().await;

    let list = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
    };
    client2.send(&list).await;
    let resp = client2.recv().await;
    match resp.msg {
        Some(proto::server_message::Msg::SessionList(sl)) => {
            assert_eq!(sl.sessions.len(), 1);
            assert_eq!(sl.sessions[0].name, "reconnect-test");
            assert_eq!(sl.sessions[0].id, session_id);
        }
        other => panic!("expected SessionList, got {other:?}"),
    }
}
