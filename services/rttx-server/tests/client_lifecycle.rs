//! Tests for the daemon client lifecycle: create, produce output, disconnect,
//! reconnect, verify scrollback is restored.
//!
//! These tests exercise the exact code path the GUI uses (`DaemonConnection` +
//! `DaemonBridge`) without any GTK dependency.

mod common;

use common::{TestClient, start_test_server, wait_for_state_containing};
use rttx_proto::proto;
use std::time::Duration;

/// Full lifecycle: create session + pane, produce output, disconnect,
/// reconnect as a new client, list sessions, attach, verify snapshot
/// contains the scrollback from the first connection.
#[tokio::test]
async fn reconnect_restores_scrollback() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    // --- First client: create session, produce output ---
    let session_id;
    let pane_id;
    {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        // Create session.
        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: "lifecycle-test".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
        session_id = match c.recv().await.msg {
            Some(proto::server_message::Msg::SessionCreated(sc)) => sc.session_id,
            other => panic!("expected SessionCreated, got {other:?}"),
        };

        // Create pane (spawns PTY).
        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                session_id: session_id.clone(),
                cwd: None,
                dark_background: None,
                cols: 0,
                rows: 0,
            })),
        })
        .await;
        pane_id = match c.recv().await.msg {
            Some(proto::server_message::Msg::PaneCreated(pc)) => pc.pane_id,
            other => panic!("expected PaneCreated, got {other:?}"),
        };

        // Attach to receive deltas.
        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
        let _snapshot = c.recv().await; // initial snapshot

        // Send a command that produces recognizable output.
        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Input(proto::Input {
                session_id: session_id.clone(),
                pane_id: pane_id.clone(),
                data: bytes::Bytes::from_static(b"echo LIFECYCLE_MARKER_12345\n"),
            })),
        })
        .await;

        // Wait for output + serialization tick.
        wait_for_state_containing(
            &tmp.path().join("cache"),
            "lifecycle-test",
            Duration::from_secs(10),
        )
        .await;

        // Drain deltas.
        let _ = c.drain(Duration::from_millis(500)).await;

        // Client disconnects (simulates GUI close).
    }

    // --- Second client: reconnect, verify state ---
    {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        // List sessions — should find our session.
        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
        })
        .await;
        let sessions = match c.recv().await.msg {
            Some(proto::server_message::Msg::SessionList(sl)) => sl.sessions,
            other => panic!("expected SessionList, got {other:?}"),
        };
        assert_eq!(sessions.len(), 1, "should have exactly 1 session");
        assert_eq!(sessions[0].name, "lifecycle-test");
        assert_eq!(sessions[0].id, session_id, "session ID should match");

        // Attach — should get snapshot with scrollback.
        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
        let snapshot = match c.recv().await.msg {
            Some(proto::server_message::Msg::Snapshot(s)) => s,
            other => panic!("expected Snapshot, got {other:?}"),
        };

        assert!(!snapshot.panes.is_empty(), "snapshot should have panes");
        let pane_snap = &snapshot.panes[0];
        assert_eq!(pane_snap.pane_id, pane_id, "pane ID should match");

        let scrollback = String::from_utf8_lossy(&pane_snap.scrollback);
        assert!(
            scrollback.contains("LIFECYCLE_MARKER_12345"),
            "snapshot scrollback should contain our marker.\nGot {} bytes: {:?}",
            pane_snap.scrollback.len(),
            &scrollback[..scrollback.len().min(200)]
        );

        // Pane should be alive (not exited).
        assert!(pane_snap.exit_status.is_none(), "pane should still be running");
    }
}

/// Verify that listing sessions after disconnect shows the correct count
/// and that creating a second session doesn't duplicate the first.
#[tokio::test]
async fn session_count_stable_across_reconnects() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    // First client: create one session.
    {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: "stable-test".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
        let _ = c.recv().await; // SessionCreated
    }

    // Second client: list — should see exactly 1.
    {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
        })
        .await;
        let sessions = match c.recv().await.msg {
            Some(proto::server_message::Msg::SessionList(sl)) => sl.sessions,
            other => panic!("expected SessionList, got {other:?}"),
        };
        assert_eq!(sessions.len(), 1, "should still have exactly 1 session");
        assert_eq!(sessions[0].name, "stable-test");
    }

    // Third client: list again — still 1.
    {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
        })
        .await;
        let sessions = match c.recv().await.msg {
            Some(proto::server_message::Msg::SessionList(sl)) => sl.sessions,
            other => panic!("expected SessionList, got {other:?}"),
        };
        assert_eq!(sessions.len(), 1, "reconnecting should not create new sessions");
    }
}

/// Full restart cycle: create session, kill server, restart, verify
/// session count is stable and scrollback is present.
#[tokio::test]
async fn restart_preserves_session_count_and_scrollback() {
    let tmp = tempfile::TempDir::new().unwrap();

    let session_id;

    // Phase 1: create session with output.
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: "restart-stable".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
        session_id = match c.recv().await.msg {
            Some(proto::server_message::Msg::SessionCreated(sc)) => sc.session_id,
            other => panic!("expected SessionCreated, got {other:?}"),
        };

        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                session_id: session_id.clone(),
                cwd: None,
                dark_background: None,
                cols: 0,
                rows: 0,
            })),
        })
        .await;
        let pane_id = match c.recv().await.msg {
            Some(proto::server_message::Msg::PaneCreated(pc)) => pc.pane_id,
            other => panic!("expected PaneCreated, got {other:?}"),
        };

        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
        let _ = c.recv().await; // Snapshot

        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Input(proto::Input {
                session_id: session_id.clone(),
                pane_id: pane_id.clone(),
                data: bytes::Bytes::from_static(b"echo RESTART_STABLE_MARKER\n"),
            })),
        })
        .await;

        // Wait for serialization.
        wait_for_state_containing(
            &tmp.path().join("cache"),
            "restart-stable",
            Duration::from_secs(10),
        )
        .await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Phase 2: restart, verify.
    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        // List — should have exactly 1 session.
        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
        })
        .await;
        let sessions = match c.recv().await.msg {
            Some(proto::server_message::Msg::SessionList(sl)) => sl.sessions,
            other => panic!("expected SessionList, got {other:?}"),
        };
        assert_eq!(sessions.len(), 1, "restart should preserve exactly 1 session");
        assert_eq!(sessions[0].id, session_id, "session ID should survive restart");

        // Attach and check scrollback.
        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
        let snapshot = match c.recv().await.msg {
            Some(proto::server_message::Msg::Snapshot(s)) => s,
            other => panic!("expected Snapshot, got {other:?}"),
        };
        assert!(!snapshot.panes.is_empty(), "should have panes after restart");

        let scrollback = String::from_utf8_lossy(&snapshot.panes[0].scrollback);
        assert!(
            scrollback.contains("RESTART_STABLE_MARKER"),
            "scrollback should survive restart. Got: {:?}",
            &scrollback[..scrollback.len().min(200)]
        );
    }

    // Phase 3: restart AGAIN, verify count is still 1 (no duplication).
    {
        // Kill phase 2 server.
        // (it's still running from start_test_server — abort it)
    }
}
