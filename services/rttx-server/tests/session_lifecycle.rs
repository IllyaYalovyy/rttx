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
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
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

#[tokio::test]
async fn rename_session_updates_name_and_inventory() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let session_id =
        common::create_session(&mut client, "original", proto::RuntimePolicy::Persistent).await;
    common::attach_rw(&mut client, &session_id).await;

    // Rename the session.
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::RenameSession(proto::RenameSession {
                session_id: session_id.clone(),
                name: "renamed".into(),
            })),
        })
        .await;

    // Expect SessionRenamed response.
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::SessionRenamed(renamed)) => {
                assert_eq!(renamed.session_id, session_id);
                assert_eq!(renamed.name, "renamed");
                break;
            }
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected SessionRenamed, got {other:?}"),
        }
    }

    // Verify inventory reflects the new name.
    let sessions = common::list_sessions(&mut client).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].name, "renamed");
}

#[tokio::test]
async fn rename_session_persists_across_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let session_id =
        common::create_session(&mut client, "before", proto::RuntimePolicy::Persistent).await;
    common::attach_rw(&mut client, &session_id).await;

    // Rename.
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::RenameSession(proto::RenameSession {
                session_id: session_id.clone(),
                name: "after".into(),
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::SessionRenamed(_)) => break,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected SessionRenamed, got {other:?}"),
        }
    }

    // Wait for state to be persisted with the new name.
    common::wait_for_state_containing(&tmp.path().join("cache"), "after", Duration::from_secs(5))
        .await;

    // Shutdown and restart.
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Shutdown(proto::Shutdown {})),
        })
        .await;
    let _ = handle.await;

    let (socket_path2, _handle2) = start_test_server(tmp.path()).await;
    let mut client2 = TestClient::connect(&socket_path2).await;
    client2.handshake().await;

    let sessions = common::list_sessions(&mut client2).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].name, "after");
}
