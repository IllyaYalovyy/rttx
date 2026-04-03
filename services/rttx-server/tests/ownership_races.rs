//! Multi-client ownership race integration tests.
//!
//! Exercises concurrent access patterns: competing writer attaches,
//! read-only clients during mutations, detach-vs-terminate races,
//! and writer disconnect during pane operations.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::proto;
use std::time::Duration;

// ── Helpers ─────────────────────────────────────────────────────

async fn create_session(client: &mut TestClient, name: &str) -> Vec<u8> {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: name.into(),
                policy: proto::RuntimePolicy::Persistent as i32,
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

async fn attach_ro(client: &mut TestClient, session_id: &[u8]) -> proto::Snapshot {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.to_vec(),
                attach_mode: proto::RuntimeAttachMode::ReadOnly as i32,
            })),
        })
        .await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::Snapshot(s)) => s,
        other => panic!("expected Snapshot, got {other:?}"),
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

async fn detach(client: &mut TestClient, session_id: &[u8]) {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::DetachSession(proto::DetachSession {
                session_id: session_id.to_vec(),
            })),
        })
        .await;
    client.recv_or_timeout().await;
}

// ── Competing writer attaches ───────────────────────────────────

#[tokio::test]
async fn three_competing_writers_only_first_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c1 = TestClient::connect(&sock).await;
    c1.handshake().await;
    let session_id = create_session(&mut c1, "race").await;
    let snap = attach_rw(&mut c1, &session_id).await;
    assert_eq!(snap.current_client_role, proto::RuntimeClientRole::Writer as i32);

    for i in 0..2 {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
        match c.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::AttachBlocked(b)) => {
                assert_eq!(b.attached_client_count, 1, "client {i}: wrong attach count");
            }
            other => panic!("client {i}: expected AttachBlocked, got {other:?}"),
        }
    }
}

// ── Read-only clients during active mutation ────────────────────

#[tokio::test]
async fn readers_observe_pane_created_push() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let session_id = create_session(&mut writer, "push-test").await;
    attach_rw(&mut writer, &session_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    attach_ro(&mut reader, &session_id).await;

    // Writer creates a pane.
    writer
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                session_id: session_id.clone(),
            })),
        })
        .await;

    // Writer gets PaneCreated response.
    let writer_resp = writer.recv_or_timeout().await;
    assert!(
        matches!(writer_resp.msg, Some(proto::server_message::Msg::PaneCreated(_))),
        "writer should get PaneCreated"
    );

    // Reader receives Delta pushes from the new pane's PTY output.
    let reader_msgs = reader.drain(Duration::from_secs(2)).await;
    assert!(
        reader_msgs.iter().any(|m| matches!(m.msg, Some(proto::server_message::Msg::Delta(_)))),
        "reader should receive Delta pushes from the new pane"
    );
}

#[tokio::test]
async fn multiple_readers_see_consistent_revision() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let session_id = create_session(&mut writer, "rev-test").await;
    let snap = attach_rw(&mut writer, &session_id).await;
    let base_rev = snap.revision;

    let mut r1 = TestClient::connect(&sock).await;
    r1.handshake().await;
    let s1 = attach_ro(&mut r1, &session_id).await;

    let mut r2 = TestClient::connect(&sock).await;
    r2.handshake().await;
    let s2 = attach_ro(&mut r2, &session_id).await;

    // Each reader attach bumps revision.
    assert!(s1.revision > base_rev);
    assert!(s2.revision > s1.revision);

    // Inventory should show consistent counts.
    let sessions = list_sessions(&mut r2).await;
    assert_eq!(sessions[0].attached_client_count, 3);
    assert_eq!(sessions[0].read_only_client_count, 2);
    assert!(sessions[0].has_write_owner);
}

// ── Detach vs terminate races ───────────────────────────────────

#[tokio::test]
async fn writer_detach_then_reader_detach_leaves_clean_state() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let session_id = create_session(&mut writer, "detach-race").await;
    attach_rw(&mut writer, &session_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    attach_ro(&mut reader, &session_id).await;

    // Writer detaches first.
    detach(&mut writer, &session_id).await;
    // Reader gets SessionDetached push.
    reader.drain(Duration::from_millis(200)).await;

    // Reader detaches.
    detach(&mut reader, &session_id).await;

    // Session should still exist (persistent policy).
    let mut checker = TestClient::connect(&sock).await;
    checker.handshake().await;
    let sessions = list_sessions(&mut checker).await;
    assert_eq!(sessions.len(), 1);
    assert!(!sessions[0].has_write_owner);
    assert_eq!(sessions[0].attached_client_count, 0);
}

#[tokio::test]
async fn terminate_while_reader_attached_notifies_reader() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let session_id = create_session(&mut writer, "term-race").await;
    attach_rw(&mut writer, &session_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    attach_ro(&mut reader, &session_id).await;

    // Writer terminates.
    writer
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::TerminateSession(proto::TerminateSession {
                session_id: session_id.clone(),
            })),
        })
        .await;

    // Both should get SessionTerminated.
    let w_resp = writer.recv_or_timeout().await;
    assert!(matches!(w_resp.msg, Some(proto::server_message::Msg::SessionTerminated(_))));

    let r_resp = reader.recv_or_timeout().await;
    assert!(matches!(r_resp.msg, Some(proto::server_message::Msg::SessionTerminated(_))));

    // Session gone.
    let mut checker = TestClient::connect(&sock).await;
    checker.handshake().await;
    let sessions = list_sessions(&mut checker).await;
    assert!(sessions.is_empty());
}

// ── Writer disconnect during pane operations ────────────────────

#[tokio::test]
async fn writer_disconnect_frees_ownership_for_new_writer() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let session_id = create_session(&mut writer, "disconnect").await;
    attach_rw(&mut writer, &session_id).await;

    // Drop the writer (simulates disconnect).
    drop(writer);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // New client should be able to attach as writer.
    let mut new_writer = TestClient::connect(&sock).await;
    new_writer.handshake().await;
    let snap = attach_rw(&mut new_writer, &session_id).await;
    assert_eq!(snap.current_client_role, proto::RuntimeClientRole::Writer as i32);
}

#[tokio::test]
async fn reader_survives_writer_disconnect() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let session_id = create_session(&mut writer, "reader-survives").await;
    attach_rw(&mut writer, &session_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    attach_ro(&mut reader, &session_id).await;

    // Drop writer.
    drop(writer);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Reader should still be able to list sessions.
    let sessions = list_sessions(&mut reader).await;
    assert_eq!(sessions.len(), 1);
    assert!(!sessions[0].has_write_owner);
    assert_eq!(sessions[0].read_only_client_count, 1);
}

// ── Revision monotonicity under concurrent operations ───────────

#[tokio::test]
async fn revisions_monotonic_across_attach_detach_cycle() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c1 = TestClient::connect(&sock).await;
    c1.handshake().await;
    let session_id = create_session(&mut c1, "mono-rev").await;

    let mut last_rev = 0u64;

    // Attach-detach cycle with multiple clients.
    for _ in 0..3 {
        let snap = attach_rw(&mut c1, &session_id).await;
        assert!(snap.revision > last_rev, "revision must increase on attach");
        last_rev = snap.revision;

        detach(&mut c1, &session_id).await;
    }

    // Final inventory check.
    let sessions = list_sessions(&mut c1).await;
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].revision >= last_rev);
}
