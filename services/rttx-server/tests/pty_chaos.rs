//! PTY chaos and state-transition tests.
//!
//! Exercises hostile timing: fast-exit shells, output bursts during close,
//! resize during/after exit, and shells that exit before first attach.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::proto;
use std::time::Duration;

// ── Helpers ─────────────────────────────────────────────────────

async fn create_and_attach(client: &mut TestClient, name: &str) -> Vec<u8> {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: name.into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let session_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::SessionCreated(sc)) => sc.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    };
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::Snapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }
    session_id
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

async fn send_input(client: &mut TestClient, session_id: &[u8], pane_id: &[u8], data: &[u8]) {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Input(proto::Input {
                session_id: session_id.to_vec(),
                pane_id: pane_id.to_vec(),
                data: data.to_vec(),
            })),
        })
        .await;
}

async fn list_sessions(client: &mut TestClient) -> Vec<proto::SessionInfo> {
    // Drain pending Deltas before sending ListSessions.
    client.drain(Duration::from_millis(100)).await;
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

/// Drain messages until we find exactly one `PaneExited` for the given pane.
async fn wait_for_pane_exited(
    client: &mut TestClient,
    expected_pane_id: &[u8],
) -> proto::PaneExited {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for PaneExited");
        match client.try_recv(remaining).await {
            Some(msg) => {
                if let Some(proto::server_message::Msg::PaneExited(pe)) = msg.msg
                    && pe.pane_id == expected_pane_id
                {
                    return pe;
                }
            }
            None => panic!("timed out waiting for PaneExited"),
        }
    }
}

// ── Fast-exit shell ─────────────────────────────────────────────

#[tokio::test]
async fn immediate_exit_command_produces_pane_exited() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let session_id = create_and_attach(&mut client, "fast-exit").await;
    let pane_id = create_pane(&mut client, &session_id).await;

    // Send `exit` to make the shell terminate immediately.
    send_input(&mut client, &session_id, &pane_id, b"exit\n").await;

    let pe = wait_for_pane_exited(&mut client, &pane_id).await;
    assert_eq!(pe.session_id, session_id);
    // Exit status varies by shell; just verify we got the notification.
}

// ── Close pane during output burst ──────────────────────────────

#[tokio::test]
async fn close_pane_during_output_burst() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let session_id = create_and_attach(&mut client, "close-burst").await;
    let pane_id = create_pane(&mut client, &session_id).await;

    // Start a burst of output.
    send_input(&mut client, &session_id, &pane_id, b"seq 1 1000\n").await;

    // Immediately close the pane while output is flowing.
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
                session_id: session_id.clone(),
                pane_id: pane_id.clone(),
            })),
        })
        .await;

    // Drain until we see PaneClosed — Deltas and PaneExited may interleave.
    let mut saw_closed = false;
    let msgs = client.drain(Duration::from_secs(5)).await;
    for msg in &msgs {
        if let Some(proto::server_message::Msg::PaneClosed(pc)) = &msg.msg
            && pc.pane_id == pane_id
        {
            saw_closed = true;
        }
    }
    assert!(saw_closed, "must receive PaneClosed after close during burst");

    // Session should still exist with 0 panes.
    let sessions = list_sessions(&mut client).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].pane_count, 0);
}

// ── Resize after pane exit ──────────────────────────────────────

#[tokio::test]
async fn resize_after_pane_exit_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let session_id = create_and_attach(&mut client, "resize-exit").await;
    let pane_id = create_pane(&mut client, &session_id).await;

    // Kill the shell.
    send_input(&mut client, &session_id, &pane_id, b"exit\n").await;
    wait_for_pane_exited(&mut client, &pane_id).await;

    // Resize the dead pane — should return an error, not panic.
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Resize(proto::Resize {
                session_id,
                pane_id,
                cols: 120,
                rows: 40,
            })),
        })
        .await;

    let resp = client.recv_or_timeout().await;
    assert!(
        matches!(resp.msg, Some(proto::server_message::Msg::Error(_))),
        "resize of exited pane must return error, got {resp:?}"
    );
}

// ── Title change interleaved with output ────────────────────────

#[tokio::test]
async fn title_change_during_output_does_not_corrupt_state() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let session_id = create_and_attach(&mut client, "title-interleave").await;
    let pane_id = create_pane(&mut client, &session_id).await;

    // Send output that includes an OSC title-change sequence interleaved with data.
    let osc_title = b"\x1b]0;my-custom-title\x07";
    let mixed = [osc_title.as_slice(), b"echo after-title\n"].concat();
    send_input(&mut client, &session_id, &pane_id, &mixed).await;

    // Let the PTY process the input.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Server should still be responsive — list sessions as a liveness check.
    let sessions = list_sessions(&mut client).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].pane_count, 1);
}

// ── Shell exits before first attach ─────────────────────────────

#[tokio::test]
async fn shell_exits_before_reattach_shows_exit_status_in_inventory() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let session_id = create_and_attach(&mut client, "pre-attach-exit").await;
    let pane_id = create_pane(&mut client, &session_id).await;

    // Send exit, then detach before the shell finishes.
    send_input(&mut client, &session_id, &pane_id, b"exit\n").await;
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::DetachSession(proto::DetachSession {
                session_id: session_id.clone(),
            })),
        })
        .await;
    client.drain(Duration::from_millis(500)).await;

    // Wait for shell to exit while detached.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Reattach — inventory should show exit status.
    let sessions = list_sessions(&mut client).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].panes.len(), 1);
    assert!(
        sessions[0].panes[0].exit_status.is_some(),
        "pane that exited while detached must report exit status"
    );
}

// ── No duplicate PaneExited ─────────────────────────────────────

#[tokio::test]
async fn no_duplicate_pane_exited_after_close_of_exited_pane() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let session_id = create_and_attach(&mut client, "no-dup-exit").await;
    let pane_id = create_pane(&mut client, &session_id).await;

    // Exit the shell.
    send_input(&mut client, &session_id, &pane_id, b"exit\n").await;
    wait_for_pane_exited(&mut client, &pane_id).await;

    // Close the already-exited pane.
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
                session_id: session_id.clone(),
                pane_id: pane_id.clone(),
            })),
        })
        .await;

    // Drain — should get PaneClosed but NOT another PaneExited.
    let msgs = client.drain(Duration::from_secs(2)).await;
    let exit_count = msgs
        .iter()
        .filter(|m| matches!(&m.msg, Some(proto::server_message::Msg::PaneExited(pe)) if pe.pane_id == pane_id))
        .count();
    assert_eq!(exit_count, 0, "must not get duplicate PaneExited after closing exited pane");
}
