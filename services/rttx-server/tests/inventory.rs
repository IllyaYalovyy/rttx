//! Integration tests for runtime inventory metadata exposed by `ListSessions`.

mod common;

use common::*;
use rttx_proto::proto;
use std::time::Duration;

#[tokio::test]
async fn list_sessions_includes_runtime_inventory_metadata() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: "inventory-test".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let session_id = match client.recv().await.msg {
        Some(proto::server_message::Msg::SessionCreated(created)) => created.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                session_id: session_id.clone(),
                cwd: None,
                dark_background: None,
                cols: 0,
                rows: 0,
            })),
        })
        .await;
    let pane_id = match client.recv().await.msg {
        Some(proto::server_message::Msg::PaneCreated(created)) => created.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    // Let the interactive shell emit its initial prompt/title traffic before
    // asserting a later manual SetPaneTitle update.
    let _ = client.drain(Duration::from_millis(500)).await;

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::SetPaneTitle(proto::SetPaneTitle {
                session_id: session_id.clone(),
                pane_id: pane_id.clone(),
                title: "inventory-shell".into(),
            })),
        })
        .await;
    assert!(matches!(client.recv().await.msg, Some(proto::server_message::Msg::TitleChanged(_))));

    let sessions = list_sessions(&mut client).await;
    assert_eq!(sessions.len(), 1);

    let session = &sessions[0];
    assert_eq!(session.id, session_id);
    assert_eq!(session.name, "inventory-test");
    assert_eq!(session.pane_count, 1);
    assert!(!session.has_attached_client);
    assert_eq!(session.attached_client_count, 0);
    assert_eq!(session.current_client_role, proto::RuntimeClientRole::Unattached as i32);
    assert!(!session.has_write_owner);
    assert_eq!(session.read_only_client_count, 0);
    assert_eq!(session.active_pane_id.as_ref(), Some(&pane_id));
    assert_eq!(
        proto::RuntimePolicy::try_from(session.policy).unwrap(),
        proto::RuntimePolicy::Persistent
    );
    assert!(!session.reconstructed);
    assert_eq!(session.panes.len(), 1);

    let pane = &session.panes[0];
    assert_eq!(pane.id, pane_id);
    assert_eq!(pane.title, "inventory-shell");
    // CWD may be populated from /proc fallback even without OSC 7.
    assert_eq!(pane.cols, 80);
    assert_eq!(pane.rows, 24);
    assert_eq!(pane.exit_status, None);
    assert!(!pane.reconstructed);
}

#[tokio::test]
async fn list_sessions_tracks_attached_client_count() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut first = TestClient::connect(&sock).await;
    first.handshake().await;

    first
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: "attach-count".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let session_id = match first.recv().await.msg {
        Some(proto::server_message::Msg::SessionCreated(created)) => created.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    first
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    assert!(matches!(first.recv().await.msg, Some(proto::server_message::Msg::Snapshot(_))));

    let mut second = TestClient::connect(&sock).await;
    second.handshake().await;
    second
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadOnly as i32,
            })),
        })
        .await;
    assert!(matches!(second.recv().await.msg, Some(proto::server_message::Msg::Snapshot(_))));

    let sessions = list_sessions(&mut second).await;
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].has_attached_client);
    assert_eq!(sessions[0].attached_client_count, 2);
    assert_eq!(sessions[0].current_client_role, proto::RuntimeClientRole::Reader as i32);
    assert!(sessions[0].has_write_owner);
    assert_eq!(sessions[0].read_only_client_count, 1);

    drop(first);
    tokio::time::sleep(Duration::from_millis(200)).await;

    let sessions = list_sessions(&mut second).await;
    assert_eq!(sessions[0].attached_client_count, 1);
    assert!(sessions[0].has_attached_client);
    assert!(!sessions[0].has_write_owner);
    assert_eq!(sessions[0].read_only_client_count, 1);

    drop(second);
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut third = TestClient::connect(&sock).await;
    third.handshake().await;
    let sessions = list_sessions(&mut third).await;
    assert_eq!(sessions[0].attached_client_count, 0);
    assert!(!sessions[0].has_attached_client);
}

#[tokio::test]
async fn list_sessions_marks_restored_runtime_and_panes_as_reconstructed() {
    let tmp = tempfile::TempDir::new().unwrap();
    let session_id;
    let pane_id;

    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                    name: "reconstructed-inventory".into(),
                    policy: proto::RuntimePolicy::Persistent as i32,
                })),
            })
            .await;
        session_id = match client.recv().await.msg {
            Some(proto::server_message::Msg::SessionCreated(created)) => created.session_id,
            other => panic!("expected SessionCreated, got {other:?}"),
        };

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                    session_id: session_id.clone(),
                    cwd: None,
                    dark_background: None,
                    cols: 0,
                    rows: 0,
                })),
            })
            .await;
        pane_id = match client.recv().await.msg {
            Some(proto::server_message::Msg::PaneCreated(created)) => created.pane_id,
            other => panic!("expected PaneCreated, got {other:?}"),
        };

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::Resize(proto::Resize {
                    session_id: session_id.clone(),
                    pane_id: pane_id.clone(),
                    cols: 100,
                    rows: 30,
                })),
            })
            .await;
        assert!(matches!(
            client.recv().await.msg,
            Some(proto::server_message::Msg::PaneResized(_))
        ));

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::SetPaneTitle(proto::SetPaneTitle {
                    session_id: session_id.clone(),
                    pane_id: pane_id.clone(),
                    title: "restored-shell".into(),
                })),
            })
            .await;
        assert!(matches!(
            client.recv().await.msg,
            Some(proto::server_message::Msg::TitleChanged(_))
        ));

        wait_for_state_containing(
            &tmp.path().join("cache"),
            "reconstructed-inventory",
            Duration::from_secs(10),
        )
        .await;

        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        let sessions = list_sessions(&mut client).await;
        assert_eq!(sessions.len(), 1);

        let session = &sessions[0];
        assert_eq!(session.id, session_id);
        assert_eq!(session.name, "reconstructed-inventory");
        assert_eq!(session.pane_count, 1);
        assert_eq!(session.active_pane_id.as_ref(), Some(&pane_id));
        assert_eq!(session.attached_client_count, 0);
        assert!(!session.has_attached_client);
        assert_eq!(session.current_client_role, proto::RuntimeClientRole::Unattached as i32);
        assert!(!session.has_write_owner);
        assert_eq!(session.read_only_client_count, 0);
        assert_eq!(
            proto::RuntimePolicy::try_from(session.policy).unwrap(),
            proto::RuntimePolicy::Persistent
        );
        assert!(session.reconstructed);
        assert_eq!(session.panes.len(), 1);

        let pane = &session.panes[0];
        assert_eq!(pane.id, pane_id);
        assert_eq!(pane.cols, 100);
        assert_eq!(pane.rows, 30);
        assert!(pane.reconstructed);
    }
}

/// Pane CWD in inventory must be populated from /proc fallback. Regression for #235.
#[tokio::test]
async fn inventory_pane_cwd_populated_from_proc_fallback() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let session_id =
        create_session(&mut client, "cwd-check", proto::RuntimePolicy::Persistent).await;
    let _pane_id = create_pane(&mut client, &session_id).await;

    // Give the shell a moment to start so /proc/<pid>/cwd is readable.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let sessions = list_sessions(&mut client).await;
    let pane = &sessions[0].panes[0];
    assert!(!pane.cwd.is_empty(), "pane CWD should be populated from /proc fallback, got empty");
}
