//! Restart and recovery behavior matrix.
//!
//! Explicit matrix covering: runtime policy × disconnect mode × client role.
//! Each test documents the expected outcome for one cell of the matrix.
//!
//! | Policy     | Disconnect Mode    | Expected After Recovery              |
//! |------------|--------------------|--------------------------------------|
//! | Persistent | Transport drop     | Session survives, reattachable       |
//! | Persistent | Daemon restart     | Session reconstructed, panes rebuilt  |
//! | Persistent | Explicit detach    | Session survives, no attached clients |
//! | Persistent | Explicit terminate | Session removed                      |
//! | Ephemeral  | Transport drop     | Session survives until restart        |
//! | Ephemeral  | Daemon restart     | Session NOT restored                 |
//! | Ephemeral  | Explicit detach    | Session terminated immediately        |

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::proto;
use std::time::Duration;

// ── Helpers ─────────────────────────────────────────────────────

async fn create_session(
    client: &mut TestClient,
    name: &str,
    policy: proto::RuntimePolicy,
) -> Vec<u8> {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: name.into(),
                policy: policy as i32,
            })),
        })
        .await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::SessionCreated(sc)) => sc.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    }
}

async fn attach_rw(client: &mut TestClient, session_id: &[u8]) -> proto::Snapshot {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.to_vec(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::Snapshot(s)) => s,
        other => panic!("expected Snapshot, got {other:?}"),
    }
}

async fn create_pane(client: &mut TestClient, session_id: &[u8]) -> Vec<u8> {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                session_id: session_id.to_vec(),
            })),
        })
        .await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    }
}

async fn list_sessions(client: &mut TestClient) -> Vec<proto::SessionInfo> {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
        })
        .await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::SessionList(sl)) => sl.sessions,
        other => panic!("expected SessionList, got {other:?}"),
    }
}

// ── Persistent × Transport disconnect ───────────────────────────

#[tokio::test]
async fn persistent_transport_drop_session_survives_and_reattaches() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let session_id = {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        let sid = create_session(&mut c, "p-drop", proto::RuntimePolicy::Persistent).await;
        attach_rw(&mut c, &sid).await;
        create_pane(&mut c, &sid).await;
        sid
        // c dropped here — transport disconnect
    };

    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut c2 = TestClient::connect(&sock).await;
    c2.handshake().await;
    let sessions = list_sessions(&mut c2).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, session_id);
    assert_eq!(sessions[0].pane_count, 1);
    assert_eq!(sessions[0].attached_client_count, 0);
    assert!(!sessions[0].has_write_owner);

    let snap = attach_rw(&mut c2, &session_id).await;
    assert_eq!(snap.current_client_role, proto::RuntimeClientRole::Writer as i32);
    assert!(!snap.panes.is_empty());
}

// ── Persistent × Daemon restart ─────────────────────────────────

#[tokio::test]
async fn persistent_daemon_restart_reconstructs_session_and_panes() {
    let tmp = tempfile::tempdir().unwrap();

    let session_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        session_id = create_session(&mut c, "p-restart", proto::RuntimePolicy::Persistent).await;
        attach_rw(&mut c, &session_id).await;
        create_pane(&mut c, &session_id).await;

        // Wait for serialization tick.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Restart.
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    let sessions = list_sessions(&mut c).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, session_id);
    assert!(sessions[0].reconstructed);
    assert_eq!(sessions[0].pane_count, 1);
    assert_eq!(sessions[0].attached_client_count, 0);

    let snap = attach_rw(&mut c, &session_id).await;
    assert_eq!(snap.panes.len(), 1);
    // reconstructed flag is on PaneInfo (inventory), not PaneSnapshot.
    assert!(snap.revision > 0);
}

// ── Persistent × Explicit detach ────────────────────────────────

#[tokio::test]
async fn persistent_explicit_detach_session_survives_unattached() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;
    let session_id = create_session(&mut c, "p-detach", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut c, &session_id).await;
    create_pane(&mut c, &session_id).await;

    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::DetachSession(proto::DetachSession {
            session_id: session_id.clone(),
        })),
    })
    .await;
    match c.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::SessionDetached(d)) => {
            assert_eq!(d.session_id, session_id);
        }
        other => panic!("expected SessionDetached, got {other:?}"),
    }

    let sessions = list_sessions(&mut c).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].attached_client_count, 0);
    assert!(!sessions[0].has_write_owner);
    assert_eq!(sessions[0].pane_count, 1);
}

// ── Persistent × Explicit terminate ─────────────────────────────

#[tokio::test]
async fn persistent_explicit_terminate_removes_session() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;
    let session_id = create_session(&mut c, "p-term", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut c, &session_id).await;

    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::TerminateSession(proto::TerminateSession {
            session_id: session_id.clone(),
        })),
    })
    .await;
    match c.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::SessionTerminated(t)) => {
            assert_eq!(t.session_id, session_id);
            assert_eq!(t.reason, proto::RuntimeTerminationReason::Explicit as i32);
        }
        other => panic!("expected SessionTerminated, got {other:?}"),
    }

    let sessions = list_sessions(&mut c).await;
    assert!(sessions.is_empty());
}

// ── Ephemeral × Transport disconnect ────────────────────────────

#[tokio::test]
async fn ephemeral_transport_drop_session_survives_until_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let session_id = {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        let sid = create_session(&mut c, "e-drop", proto::RuntimePolicy::Ephemeral).await;
        attach_rw(&mut c, &sid).await;
        sid
    };

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Session still exists after transport drop (not explicit detach).
    let mut c2 = TestClient::connect(&sock).await;
    c2.handshake().await;
    let sessions = list_sessions(&mut c2).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, session_id);
    assert_eq!(
        proto::RuntimePolicy::try_from(sessions[0].policy).unwrap(),
        proto::RuntimePolicy::Ephemeral
    );
}

// ── Ephemeral × Daemon restart ──────────────────────────────────

#[tokio::test]
async fn ephemeral_daemon_restart_does_not_restore_session() {
    let tmp = tempfile::tempdir().unwrap();

    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        let sid = create_session(&mut c, "e-restart", proto::RuntimePolicy::Ephemeral).await;
        attach_rw(&mut c, &sid).await;
        create_pane(&mut c, &sid).await;

        tokio::time::sleep(Duration::from_millis(1500)).await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    let sessions = list_sessions(&mut c).await;
    assert!(sessions.is_empty(), "ephemeral sessions must not survive restart");
}

// ── Ephemeral × Explicit detach ─────────────────────────────────

#[tokio::test]
async fn ephemeral_explicit_detach_terminates_immediately() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;
    let session_id = create_session(&mut c, "e-detach", proto::RuntimePolicy::Ephemeral).await;
    attach_rw(&mut c, &session_id).await;

    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::DetachSession(proto::DetachSession {
            session_id: session_id.clone(),
        })),
    })
    .await;
    match c.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::SessionTerminated(t)) => {
            assert_eq!(t.session_id, session_id);
            assert_eq!(t.reason, proto::RuntimeTerminationReason::EphemeralLastDetach as i32);
        }
        other => panic!("expected SessionTerminated, got {other:?}"),
    }

    let sessions = list_sessions(&mut c).await;
    assert!(sessions.is_empty());
}

// ── Persistent × Restart with read-only client role ─────────────

#[tokio::test]
async fn persistent_restart_reader_reattaches_after_reconstruction() {
    let tmp = tempfile::tempdir().unwrap();

    let session_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut writer = TestClient::connect(&sock).await;
        writer.handshake().await;
        session_id =
            create_session(&mut writer, "p-reader-restart", proto::RuntimePolicy::Persistent).await;
        attach_rw(&mut writer, &session_id).await;
        create_pane(&mut writer, &session_id).await;

        tokio::time::sleep(Duration::from_millis(1500)).await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;

    // Attach as read-only after reconstruction.
    reader
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadOnly as i32,
            })),
        })
        .await;
    match reader.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::Snapshot(snap)) => {
            assert_eq!(snap.current_client_role, proto::RuntimeClientRole::Reader as i32);
            assert!(!snap.panes.is_empty());
            // reconstructed flag is on PaneInfo (inventory), not PaneSnapshot.
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    let sessions = list_sessions(&mut reader).await;
    assert_eq!(sessions[0].read_only_client_count, 1);
    assert!(!sessions[0].has_write_owner);
}
