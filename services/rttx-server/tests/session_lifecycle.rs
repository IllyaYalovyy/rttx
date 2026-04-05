//! Integration tests for session lifecycle.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::proto;
use std::time::Duration;

#[tokio::test]
async fn create_session_and_list() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    // Create a session.
    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
            name: "test-session".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let resp = client.recv().await;
    let session_id = match resp.msg {
        Some(proto::server_message::Msg::SessionCreated(sc)) => sc.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    };
    assert_eq!(session_id.len(), 16);

    // List sessions.
    let list = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
    };
    client.send(&list).await;
    let resp = client.recv().await;
    match resp.msg {
        Some(proto::server_message::Msg::SessionList(sl)) => {
            assert_eq!(sl.sessions.len(), 1);
            assert_eq!(sl.sessions[0].name, "test-session");
        }
        other => panic!("expected SessionList, got {other:?}"),
    }
}

#[tokio::test]
async fn attach_and_detach_session() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    // Create session.
    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
            name: "attach-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let resp = client.recv().await;
    let session_id = match resp.msg {
        Some(proto::server_message::Msg::SessionCreated(sc)) => sc.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    // Attach.
    let attach = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
            session_id: session_id.clone(),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    let resp = client.recv().await;
    match resp.msg {
        Some(proto::server_message::Msg::Snapshot(snap)) => {
            assert_eq!(snap.session_id, session_id);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    // Detach.
    let detach = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::DetachSession(proto::DetachSession {
            session_id: session_id.clone(),
        })),
    };
    client.send(&detach).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for SessionDetached");
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::SessionDetached(_)) => break,
            Some(
                proto::server_message::Msg::Delta(_) | proto::server_message::Msg::PaneExited(_),
            ) => {}
            other => panic!("expected SessionDetached, got {other:?}"),
        }
    }

    // Verify session still exists after detach.
    let list = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
    };
    client.send(&list).await;
    let resp = client.recv().await;
    match resp.msg {
        Some(proto::server_message::Msg::SessionList(sl)) => {
            assert_eq!(sl.sessions.len(), 1);
        }
        other => panic!("expected SessionList, got {other:?}"),
    }
}

#[tokio::test]
async fn create_and_close_pane() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    // Create session.
    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
            name: "pane-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let resp = client.recv().await;
    let session_id = match resp.msg {
        Some(proto::server_message::Msg::SessionCreated(sc)) => sc.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    // Create pane.
    let create_pane = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
            session_id: session_id.clone(),
        })),
    };
    client.send(&create_pane).await;
    let resp = client.recv().await;
    let pane_id = match resp.msg {
        Some(proto::server_message::Msg::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    // Close pane.
    let close_pane = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
            session_id: session_id.clone(),
            pane_id,
        })),
    };
    client.send(&close_pane).await;
    let resp = client.recv().await;
    assert!(matches!(resp.msg, Some(proto::server_message::Msg::PaneClosed(_))));
}
