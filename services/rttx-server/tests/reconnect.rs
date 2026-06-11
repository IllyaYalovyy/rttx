//! Integration tests for client reconnection and error recovery.
//!
//! Covers: rapid reconnect storms, reconnect during active PTY output,
//! reconnect to terminated sessions, pane visibility across reconnects,
//! revision monotonicity, post-detach error handling, delta delivery
//! after reattach, and concurrent multi-client attach.

mod common;

use common::{
    TestClient, attach_ro, attach_rw, create_pane, create_workspace, detach_workspace, list_workspaces,
    send_input, start_test_server, terminate_workspace,
};
use rttx_proto::v3;
use std::time::Duration;

#[tokio::test]
async fn reconnect_after_disconnect() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let runtime_id = {
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;
        create_workspace(&mut client, "reconnect-test", v3::WorkspacePolicy::Persistent).await
    };

    let mut client2 = TestClient::connect(&sock).await;
    client2.handshake().await;
    let workspaces = list_workspaces(&mut client2).await;
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].name, "reconnect-test");
    assert_eq!(workspaces[0].id, runtime_id);
}

/// Five clients connect, attach, detach, and disconnect in rapid succession.
/// The persistent session must survive with all panes intact.
#[tokio::test]
async fn rapid_reconnect_storm() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let runtime_id = {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        let sid = create_workspace(&mut c, "storm", v3::WorkspacePolicy::Persistent).await;
        attach_rw(&mut c, &sid).await;
        create_pane(&mut c, &sid).await;
        detach_workspace(&mut c, &sid).await;
        sid
    };

    for i in 0..5 {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        let snap = attach_rw(&mut c, &runtime_id).await;
        assert!(!snap.panes.is_empty(), "reconnect {i}: session should have panes");
        detach_workspace(&mut c, &runtime_id).await;
    }

    let mut final_client = TestClient::connect(&sock).await;
    final_client.handshake().await;
    let workspaces = list_workspaces(&mut final_client).await;
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].pane_count, 1);
}

/// Client disconnects while PTY is producing output. A new client reattaches
/// and receives a snapshot containing the accumulated scrollback.
#[tokio::test]
async fn reconnect_during_active_pty_output() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let (runtime_id, pane_id) = {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        let sid = create_workspace(&mut c, "active-output", v3::WorkspacePolicy::Persistent).await;
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
    let snap = attach_rw(&mut c2, &runtime_id).await;
    let pane_snap = snap.panes.iter().find(|p| p.pane_id == pane_id);
    assert!(pane_snap.is_some(), "pane should be in snapshot");
    let scrollback = String::from_utf8_lossy(&pane_snap.unwrap().scrollback_tail);
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

    let runtime_id = {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        create_workspace(&mut c, "doomed", v3::WorkspacePolicy::Persistent).await
    };

    // Another client terminates the session.
    {
        let mut c2 = TestClient::connect(&sock).await;
        c2.handshake().await;
        terminate_workspace(&mut c2, &runtime_id).await;
    }

    // Original client reconnects and tries to attach.
    let mut c3 = TestClient::connect(&sock).await;
    c3.handshake().await;
    c3.send(&v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
            runtime_id: runtime_id.clone(),
            attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
        })),
    })
    .await;
    let resp = c3.recv().await;
    match resp.payload {
        Some(v3::server_envelope::Payload::Error(e)) => {
            assert_eq!(e.kind, 4, "should be ERR_SESSION_NOT_FOUND");
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
    let sid = create_workspace(&mut client_a, "multi-pane", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client_a, &sid).await;
    let pane1 = create_pane(&mut client_a, &sid).await;
    let pane2 = create_pane(&mut client_a, &sid).await;
    detach_workspace(&mut client_a, &sid).await;

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

    let sid = create_workspace(&mut c, "rev-test", v3::WorkspacePolicy::Persistent).await;
    let snap1 = attach_rw(&mut c, &sid).await;
    let rev_after_attach = snap1.workspace_revision;

    create_pane(&mut c, &sid).await;
    detach_workspace(&mut c, &sid).await;

    // Reconnect.
    let mut c2 = TestClient::connect(&sock).await;
    c2.handshake().await;
    let snap2 = attach_rw(&mut c2, &sid).await;
    let rev_after_reattach = snap2.workspace_revision;

    assert!(
        rev_after_reattach > rev_after_attach,
        "revision should increase: {rev_after_attach} -> {rev_after_reattach}"
    );

    create_pane(&mut c2, &sid).await;
    detach_workspace(&mut c2, &sid).await;

    let mut c3 = TestClient::connect(&sock).await;
    c3.handshake().await;
    let snap3 = attach_rw(&mut c3, &sid).await;
    assert!(
        snap3.workspace_revision > rev_after_reattach,
        "revision should keep increasing: {rev_after_reattach} -> {}",
        snap3.workspace_revision
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
    let sid = create_workspace(&mut c1, "detach-ops", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut c1, &sid).await;
    let pane_id = create_pane(&mut c1, &sid).await;
    detach_workspace(&mut c1, &sid).await;

    // Another client takes ownership.
    let mut c2 = TestClient::connect(&sock).await;
    c2.handshake().await;
    attach_rw(&mut c2, &sid).await;

    // Original client tries to close pane — should fail with ownership error.
    c1.send(&v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::ClosePane(v3::ClosePane {
            runtime_id: sid.clone(),
            pane_id: pane_id.clone(),
        })),
    })
    .await;
    let resp = c1.recv().await;
    match resp.payload {
        Some(v3::server_envelope::Payload::Error(e)) => {
            assert_eq!(
                e.kind,
                v3::ErrorKind::OwnershipConflict as i32,
                "should be ERR_OWNERSHIP_CONFLICT"
            );
        }
        other => panic!("expected Error for close-pane while another writer owns, got {other:?}"),
    }

    // Original client tries to resize — ResizePane is fire-and-forget, so an
    // unauthorized attempt is silently dropped (no error, no effect) rather
    // than rejected. Verify no error comes back after a Ping/Pong barrier.
    c1.send(&v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::ResizePane(v3::ResizePane {
            runtime_id: sid.clone(),
            pane_id,
            cols: 120,
            rows: 40,
        })),
    })
    .await;
    c1.ping().await;
    let events = c1.drain(std::time::Duration::from_millis(200)).await;
    assert!(
        events.iter().all(|e| !matches!(e.payload, Some(v3::server_envelope::Payload::Error(_)))),
        "unauthorized resize must be silently dropped, not errored"
    );
}

/// After reattach, new deltas from PTY output are delivered to the
/// reconnected client.
#[tokio::test]
async fn reconnect_receives_delta_stream_from_active_panes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c1 = TestClient::connect(&sock).await;
    c1.handshake().await;
    let sid = create_workspace(&mut c1, "delta-stream", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut c1, &sid).await;
    let pane_id = create_pane(&mut c1, &sid).await;
    detach_workspace(&mut c1, &sid).await;
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
        .filter_map(|m| match &m.payload {
            Some(v3::server_envelope::Payload::OutputDelta(d)) if d.pane_id == pane_id => {
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
    let sid = create_workspace(&mut c1, "concurrent", v3::WorkspacePolicy::Persistent).await;
    let _snap = attach_rw(&mut c1, &sid).await;

    // Second client tries to attach as writer — should be blocked.
    let mut c2 = TestClient::connect(&sock).await;
    c2.handshake().await;
    c2.send(&v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
            runtime_id: sid.clone(),
            attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
        })),
    })
    .await;
    let resp = c2.recv().await;
    match resp.payload {
        Some(v3::server_envelope::Payload::AttachBlocked(ab)) => {
            assert_eq!(ab.runtime_id, sid);
            assert!(ab.attached_client_count >= 1);
        }
        other => panic!("expected AttachBlocked for second writer, got {other:?}"),
    }

    // Second client can still attach as read-only.
    let snap = attach_ro(&mut c2, &sid).await;
    assert_eq!(snap.runtime_id, sid);
}
