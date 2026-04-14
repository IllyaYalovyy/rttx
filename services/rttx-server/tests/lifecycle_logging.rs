//! Integration test: lifecycle events produce correct protocol responses
//! through the full server path (the same path that now emits log messages).

mod common;

use common::{
    attach_rw, close_pane, create_pane, create_session, detach_session, start_test_server,
    terminate_session,
};
use rttx_proto::proto;

#[tokio::test]
async fn full_lifecycle_produces_expected_responses() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;

    let mut client = common::TestClient::connect(&socket_path).await;
    client.handshake().await;

    // Create → attach → create pane → close pane → detach → terminate.
    let sid =
        create_session(&mut client, "lifecycle-log-test", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut client, &sid).await;
    let pane_id = create_pane(&mut client, &sid).await;
    close_pane(&mut client, &sid, &pane_id).await;
    detach_session(&mut client, &sid).await;

    // Re-attach to terminate (need write access).
    attach_rw(&mut client, &sid).await;
    terminate_session(&mut client, &sid).await;

    // Session should be gone.
    let sessions = common::list_sessions(&mut client).await;
    assert!(sessions.is_empty(), "session should be removed after terminate");
}

#[tokio::test]
async fn rename_session_through_server() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;

    let mut client = common::TestClient::connect(&socket_path).await;
    client.handshake().await;

    let sid = create_session(&mut client, "before-rename", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut client, &sid).await;

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::RenameSession(proto::RenameSession {
                session_id: sid.clone(),
                name: "after-rename".into(),
            })),
        })
        .await;

    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::SessionRenamed(sr)) => {
                assert_eq!(sr.name, "after-rename");
                break;
            }
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected SessionRenamed, got {other:?}"),
        }
    }

    let sessions = common::list_sessions(&mut client).await;
    assert_eq!(sessions[0].name, "after-rename");
}
