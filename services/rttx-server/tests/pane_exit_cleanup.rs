//! Integration test: server emits terminal cleanup sequence on pane process exit.
//!
//! Verifies that when a pane's leaf process dies, the server feeds the
//! cleanup byte sequence into the pane's screen state and broadcasts it
//! to attached clients. This ensures reconnecting clients see a clean
//! terminal (alt-screen off, cursor visible, mouse off, etc.).

mod common;

use common::*;
use rttx_proto::proto;
use std::time::Duration;

/// Collect all Delta data bytes received before `PaneExited`.
async fn collect_deltas_until_exit(
    client: &mut TestClient,
    timeout: Duration,
) -> (Vec<u8>, Option<proto::PaneExited>) {
    let mut all_data = Vec::new();
    let mut exit_msg = None;
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let Some(msg) = client.try_recv(remaining).await else { break };
        match msg.msg {
            Some(proto::server_message::Msg::Delta(d)) => {
                all_data.extend_from_slice(&d.data);
            }
            Some(proto::server_message::Msg::PaneExited(pe)) => {
                exit_msg = Some(pe);
                break;
            }
            _ => {}
        }
    }
    (all_data, exit_msg)
}

#[tokio::test]
async fn cleanup_sequence_broadcast_on_pane_exit() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let sid = create_runtime(&mut client, "cleanup-test", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut client, &sid).await;
    let pane_id = create_pane(&mut client, &sid).await;

    // Send "exit" to make the shell terminate.
    send_input(&mut client, &sid, &pane_id, b"exit\n").await;

    let (delta_data, exit_msg) =
        collect_deltas_until_exit(&mut client, Duration::from_secs(15)).await;

    assert!(exit_msg.is_some(), "should receive PaneExited");

    // The cleanup sequence should appear in the delta stream before PaneExited.
    let cleanup = rttx_server::screen::terminal_cleanup_bytes();
    assert!(
        delta_data.windows(cleanup.len()).any(|w| w == cleanup),
        "delta stream should contain the terminal cleanup sequence"
    );
}

#[tokio::test]
async fn reattach_after_exit_sees_clean_terminal_modes() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let sid = create_runtime(&mut client, "reattach-test", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut client, &sid).await;
    let pane_id = create_pane(&mut client, &sid).await;

    // Send "exit" to make the shell terminate.
    send_input(&mut client, &sid, &pane_id, b"exit\n").await;

    // Wait for PaneExited.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for PaneExited");
        if let Some(msg) = client.try_recv(remaining).await
            && matches!(msg.msg, Some(proto::server_message::Msg::PaneExited(_)))
        {
            break;
        }
    }

    // Detach and reattach to get a fresh snapshot.
    detach_runtime(&mut client, &sid).await;
    let snapshot = attach_rw(&mut client, &sid).await;

    let pane = snapshot
        .panes
        .iter()
        .find(|p| p.pane_id == pane_id)
        .expect("pane should still be in snapshot");

    // After cleanup, all TUI modes should be off.
    assert!(!pane.bracketed_paste_mode, "bracketed paste should be off");
    assert!(!pane.application_cursor_keys, "application cursor keys should be off");
    assert!(!pane.application_keypad, "application keypad should be off");
    assert_eq!(pane.mouse_tracking_mode, 0, "mouse tracking should be off");
    assert!(!pane.sgr_mouse_mode, "SGR mouse should be off");
}
