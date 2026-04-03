//! Resource-leak loop tests for runtime lifecycle.
//!
//! Repeated create/attach/detach/terminate cycles that verify the server
//! returns to a stable steady state with no leaked runtimes or panes.
//! No sleep-based timing — all assertions use polling with timeouts.

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

async fn attach_rw(client: &mut TestClient, session_id: &[u8]) {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.to_vec(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::Snapshot(_)) => {}
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
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::PaneCreated(pc)) => return pc.pane_id,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected PaneCreated, got {other:?}"),
        }
    }
}

async fn close_pane(client: &mut TestClient, session_id: &[u8], pane_id: &[u8]) {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
                session_id: session_id.to_vec(),
                pane_id: pane_id.to_vec(),
            })),
        })
        .await;
    // Drain until PaneClosed — Deltas and PaneExited may interleave.
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::PaneClosed(_)) => return,
            Some(
                proto::server_message::Msg::Delta(_) | proto::server_message::Msg::PaneExited(_),
            ) => {}
            other => panic!("expected PaneClosed, got {other:?}"),
        }
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
    // Drain Deltas until we get SessionDetached or SessionTerminated.
    loop {
        match client.recv_or_timeout().await.msg {
            Some(
                proto::server_message::Msg::SessionDetached(_)
                | proto::server_message::Msg::SessionTerminated(_),
            ) => return,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected SessionDetached/Terminated, got {other:?}"),
        }
    }
}

async fn terminate(client: &mut TestClient, session_id: &[u8]) {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::TerminateSession(proto::TerminateSession {
                session_id: session_id.to_vec(),
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::SessionTerminated(_)) => return,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected SessionTerminated, got {other:?}"),
        }
    }
}

async fn list_sessions(client: &mut TestClient) -> Vec<proto::SessionInfo> {
    client.drain(Duration::from_millis(50)).await;
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::SessionList(sl)) => return sl.sessions,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected SessionList, got {other:?}"),
        }
    }
}

// ── Create-terminate loop ───────────────────────────────────────

#[tokio::test]
async fn create_terminate_loop_leaves_zero_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    for i in 0..10 {
        let sid =
            create_session(&mut client, &format!("loop-{i}"), proto::RuntimePolicy::Persistent)
                .await;
        attach_rw(&mut client, &sid).await;
        terminate(&mut client, &sid).await;
    }

    let sessions = list_sessions(&mut client).await;
    assert_eq!(sessions.len(), 0, "all terminated sessions must be cleaned up");
}

// ── Create-pane-close-pane loop ─────────────────────────────────

#[tokio::test]
async fn create_close_pane_loop_returns_to_zero_panes() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let sid = create_session(&mut client, "pane-loop", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut client, &sid).await;

    for _ in 0..10 {
        let pane_id = create_pane(&mut client, &sid).await;
        close_pane(&mut client, &sid, &pane_id).await;
    }

    let sessions = list_sessions(&mut client).await;
    assert_eq!(sessions[0].pane_count, 0, "all closed panes must be cleaned up");
}

// ── Attach-detach loop on persistent runtime ────────────────────

#[tokio::test]
async fn attach_detach_loop_persistent_session_survives() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let sid = create_session(&mut client, "detach-loop", proto::RuntimePolicy::Persistent).await;

    for _ in 0..10 {
        attach_rw(&mut client, &sid).await;
        detach(&mut client, &sid).await;
    }

    let sessions = list_sessions(&mut client).await;
    assert_eq!(sessions.len(), 1, "persistent session must survive detach loops");
    assert_eq!(sessions[0].attached_client_count, 0);
    assert!(!sessions[0].has_write_owner);
}

// ── Ephemeral create-attach-detach loop ─────────────────────────

#[tokio::test]
async fn ephemeral_create_detach_loop_leaves_zero_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    for i in 0..10 {
        let sid =
            create_session(&mut client, &format!("eph-{i}"), proto::RuntimePolicy::Ephemeral).await;
        attach_rw(&mut client, &sid).await;
        detach(&mut client, &sid).await;
    }

    let sessions = list_sessions(&mut client).await;
    assert_eq!(sessions.len(), 0, "ephemeral sessions must terminate on last detach");
}

// ── Full lifecycle loop ─────────────────────────────────────────

#[tokio::test]
async fn full_lifecycle_loop_returns_to_clean_state() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    for i in 0..5 {
        let sid =
            create_session(&mut client, &format!("full-{i}"), proto::RuntimePolicy::Persistent)
                .await;
        attach_rw(&mut client, &sid).await;
        let p1 = create_pane(&mut client, &sid).await;
        let p2 = create_pane(&mut client, &sid).await;
        close_pane(&mut client, &sid, &p1).await;
        close_pane(&mut client, &sid, &p2).await;
        terminate(&mut client, &sid).await;
    }

    let sessions = list_sessions(&mut client).await;
    assert_eq!(sessions.len(), 0, "full lifecycle loop must leave zero sessions");
}

// ── Reconnect loop with transport disconnect ────────────────────

#[tokio::test]
async fn reconnect_loop_does_not_leak_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    // Create one persistent session.
    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;
    let sid = create_session(&mut c, "reconnect-loop", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut c, &sid).await;
    drop(c);

    // Reconnect 10 times — session count must stay at 1.
    for _ in 0..10 {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        let sessions = list_sessions(&mut c).await;
        assert_eq!(sessions.len(), 1, "reconnect must not create duplicate sessions");
        attach_rw(&mut c, &sid).await;
        drop(c);
    }

    // Final check.
    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;
    let sessions = list_sessions(&mut c).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].attached_client_count, 0);
}
