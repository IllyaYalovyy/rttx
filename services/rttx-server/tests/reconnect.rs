//! Integration tests for client reconnection and error recovery.
//!
//! Covers: rapid reconnect storms, reconnect during active PTY output,
//! reconnect to terminated sessions, pane visibility across reconnects,
//! revision monotonicity, post-detach error handling, delta delivery
//! after reattach, and concurrent multi-client attach.

mod common;

use common::{
    TestClient, attach_ro, attach_rw, create_pane, create_session, detach_session, list_sessions,
    send_input, start_test_server, terminate_session,
};
use rttx_proto::proto;
use std::time::Duration;

#[tokio::test]
async fn reconnect_after_disconnect() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let session_id = {
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;
        create_session(&mut client, "reconnect-test", proto::RuntimePolicy::Persistent).await
    };

    let mut client2 = TestClient::connect(&sock).await;
    client2.handshake().await;
    let sessions = list_sessions(&mut client2).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].name, "reconnect-test");
    assert_eq!(sessions[0].id, session_id);
}

/// Five clients connect, attach, detach, and disconnect in rapid succession.
/// The persistent session must survive with all panes intact.
#[tokio::test]
async fn rapid_reconnect_storm() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let session_id = {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        let sid = create_session(&mut c, "storm", proto::RuntimePolicy::Persistent).await;
        attach_rw(&mut c, &sid).await;
        create_pane(&mut c, &sid).await;
        detach_session(&mut c, &sid).await;
        sid
    };

    for i in 0..5 {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        let snap = attach_rw(&mut c, &session_id).await;
        assert!(!snap.panes.is_empty(), "reconnect {i}: session should have panes");
        detach_session(&mut c, &session_id).await;
    }

    let mut final_client = TestClient::connect(&sock).await;
    final_client.handshake().await;
    let sessions = list_sessions(&mut final_client).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].pane_count, 1);
}

/// Client disconnects while PTY is producing output. A new client reattaches
/// and receives a snapshot containing the accumulated scrollback.
#[tokio::test]
async fn reconnect_during_active_pty_output() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let (session_id, pane_id) = {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        let sid = create_session(&mut c, "active-output", proto::RuntimePolicy::Persistent).await;
        attach_rw(&mut c, &sid).await;
        let pid = create_pane(&mut c, &sid).await;
        // Send a command that produces output.
        send_input(&mut c, &sid, &pid, b"echo MARKER_RECONNECT_TEST\n").await;
        // Give PTY time to produce output.
        tokio::time::sleep(Duration::from_millis(500)).await;
        (sid, pid)
        // client drops while PTY is active
    };

    // Reconnect and verify scrollback contains the marker.
    let mut c2 = TestClient::connect(&sock).await;
    c2.handshake().await;
    let snap = attach_rw(&mut c2, &session_id).await;
    let pane_snap = snap.panes.iter().find(|p| p.pane_id == pane_id);
    assert!(pane_snap.is_some(), "pane should be in snapshot");
    let scrollback = String::from_utf8_lossy(&pane_snap.unwrap().scrollback);
    assert!(
        scrollback.contains("MARKER_RECONNECT_TEST"),
        "scrollback should contain output produced while disconnected, got: {scrollback}"
    );
}

/// Session is terminated between disconnect and reconnect. Attach returns
/// `session_not_found`.
#[tokio::test]
async fn reconnect_to_terminated_session_returns_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let session_id = {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        create_session(&mut c, "doomed", proto::RuntimePolicy::Persistent).await
    };

    // Another client terminates the session.
    {
        let mut c2 = TestClient::connect(&sock).await;
        c2.handshake().await;
        terminate_session(&mut c2, &session_id).await;
    }

    // Original client reconnects and tries to attach.
    let mut c3 = TestClient::connect(&sock).await;
    c3.handshake().await;
    c3.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
            session_id: session_id.clone(),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    })
    .await;
    let resp = c3.recv().await;
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, 4, "should be ERR_SESSION_NOT_FOUND");
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

/// Client A adds panes. Client B reconnects and sees them in the snapshot.
#[tokio::test]
async fn reconnect_sees_panes_added_by_other_client() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client_a = TestClient::connect(&sock).await;
    client_a.handshake().await;
    let sid = create_session(&mut client_a, "multi-pane", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut client_a, &sid).await;
    let pane1 = create_pane(&mut client_a, &sid).await;
    let pane2 = create_pane(&mut client_a, &sid).await;
    detach_session(&mut client_a, &sid).await;

    // Client B reconnects and should see both panes.
    let mut client_b = TestClient::connect(&sock).await;
    client_b.handshake().await;
    let snap = attach_rw(&mut client_b, &sid).await;
    assert_eq!(snap.panes.len(), 2, "snapshot should contain both panes");
    let ids: Vec<_> = snap.panes.iter().map(|p| p.pane_id.clone()).collect();
    assert!(ids.contains(&pane1));
    assert!(ids.contains(&pane2));
}

/// Revision monotonically increases through create → attach → pane-add →
/// detach → reattach cycle.
#[tokio::test]
async fn revision_increases_across_reconnect_cycles() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    let sid = create_session(&mut c, "rev-test", proto::RuntimePolicy::Persistent).await;
    let snap1 = attach_rw(&mut c, &sid).await;
    let rev_after_attach = snap1.revision;

    create_pane(&mut c, &sid).await;
    detach_session(&mut c, &sid).await;

    // Reconnect.
    let mut c2 = TestClient::connect(&sock).await;
    c2.handshake().await;
    let snap2 = attach_rw(&mut c2, &sid).await;
    let rev_after_reattach = snap2.revision;

    assert!(
        rev_after_reattach > rev_after_attach,
        "revision should increase: {rev_after_attach} -> {rev_after_reattach}"
    );

    create_pane(&mut c2, &sid).await;
    detach_session(&mut c2, &sid).await;

    let mut c3 = TestClient::connect(&sock).await;
    c3.handshake().await;
    let snap3 = attach_rw(&mut c3, &sid).await;
    assert!(
        snap3.revision > rev_after_reattach,
        "revision should keep increasing: {rev_after_reattach} -> {}",
        snap3.revision
    );
}

/// After detaching, a client cannot mutate a session that another client
/// owns as writer.
#[tokio::test]
async fn operations_after_detach_blocked_by_other_writer() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c1 = TestClient::connect(&sock).await;
    c1.handshake().await;
    let sid = create_session(&mut c1, "detach-ops", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut c1, &sid).await;
    let pane_id = create_pane(&mut c1, &sid).await;
    detach_session(&mut c1, &sid).await;

    // Another client takes ownership.
    let mut c2 = TestClient::connect(&sock).await;
    c2.handshake().await;
    attach_rw(&mut c2, &sid).await;

    // Original client tries to close pane — should fail with ownership error.
    c1.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
            session_id: sid.clone(),
            pane_id: pane_id.clone(),
        })),
    })
    .await;
    let resp = c1.recv().await;
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, 9, "should be ERR_OWNERSHIP_CONFLICT");
        }
        other => panic!("expected Error for close-pane while another writer owns, got {other:?}"),
    }

    // Original client tries to resize — should also fail.
    c1.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Resize(proto::Resize {
            session_id: sid.clone(),
            pane_id,
            cols: 120,
            rows: 40,
        })),
    })
    .await;
    let resp = c1.recv().await;
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, 9, "should be ERR_OWNERSHIP_CONFLICT");
        }
        other => panic!("expected Error for resize while another writer owns, got {other:?}"),
    }
}

/// After reattach, new deltas from PTY output are delivered to the
/// reconnected client.
#[tokio::test]
async fn reconnect_receives_delta_stream_from_active_panes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c1 = TestClient::connect(&sock).await;
    c1.handshake().await;
    let sid = create_session(&mut c1, "delta-stream", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut c1, &sid).await;
    let pane_id = create_pane(&mut c1, &sid).await;
    detach_session(&mut c1, &sid).await;
    drop(c1);

    // Reconnect.
    let mut c2 = TestClient::connect(&sock).await;
    c2.handshake().await;
    attach_rw(&mut c2, &sid).await;

    // Send input that produces output.
    send_input(&mut c2, &sid, &pane_id, b"echo DELTA_MARKER_42\n").await;

    // Collect deltas for a short window.
    let msgs = c2.drain(Duration::from_secs(2)).await;
    let delta_data: Vec<u8> = msgs
        .iter()
        .filter_map(|m| match &m.msg {
            Some(proto::server_message::Msg::Delta(d)) if d.pane_id == pane_id => {
                Some(d.data.clone())
            }
            _ => None,
        })
        .flatten()
        .collect();
    let delta_text = String::from_utf8_lossy(&delta_data);
    assert!(
        delta_text.contains("DELTA_MARKER_42"),
        "should receive delta with PTY output after reattach, got: {delta_text}"
    );
}

/// Two clients attach simultaneously to the same session. The second writer
/// gets blocked.
#[tokio::test]
async fn concurrent_reconnect_two_clients_same_session() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c1 = TestClient::connect(&sock).await;
    c1.handshake().await;
    let sid = create_session(&mut c1, "concurrent", proto::RuntimePolicy::Persistent).await;
    let _snap = attach_rw(&mut c1, &sid).await;

    // Second client tries to attach as writer — should be blocked.
    let mut c2 = TestClient::connect(&sock).await;
    c2.handshake().await;
    c2.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
            session_id: sid.clone(),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    })
    .await;
    let resp = c2.recv().await;
    match resp.msg {
        Some(proto::server_message::Msg::AttachBlocked(ab)) => {
            assert_eq!(ab.session_id, sid);
            assert!(ab.attached_client_count >= 1);
        }
        other => panic!("expected AttachBlocked for second writer, got {other:?}"),
    }

    // Second client can still attach as read-only.
    let snap = attach_ro(&mut c2, &sid).await;
    assert_eq!(snap.session_id, sid);
}
