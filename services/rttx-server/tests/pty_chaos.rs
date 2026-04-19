//! PTY chaos and state-transition tests.
//!
//! Exercises hostile timing: fast-exit shells, output bursts during close,
//! resize during/after exit, and shells that exit before first attach.

mod common;

use common::*;
use rttx_proto::proto;
use std::time::Duration;

// ── Helpers ─────────────────────────────────────────────────────

async fn create_and_attach(client: &mut TestClient, name: &str) -> Vec<u8> {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
                name: name.into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
                runtime_id: runtime_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::Snapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }
    runtime_id
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
    let runtime_id = create_and_attach(&mut client, "fast-exit").await;
    let pane_id = create_pane(&mut client, &runtime_id).await;

    // Send `exit` to make the shell terminate immediately.
    send_input(&mut client, &runtime_id, &pane_id, b"exit\n").await;

    let pe = wait_for_pane_exited(&mut client, &pane_id).await;
    assert_eq!(pe.runtime_id, runtime_id);
    // Exit status varies by shell; just verify we got the notification.
}

// ── Close pane during output burst ──────────────────────────────

#[tokio::test]
async fn close_pane_during_output_burst() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let runtime_id = create_and_attach(&mut client, "close-burst").await;
    let pane_id = create_pane(&mut client, &runtime_id).await;

    // Start a burst of output.
    send_input(&mut client, &runtime_id, &pane_id, b"seq 1 1000\n").await;

    // Immediately close the pane while output is flowing.
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
                runtime_id: runtime_id.clone(),
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
    let runtimes = list_runtimes(&mut client).await;
    assert_eq!(runtimes.len(), 1);
    assert_eq!(runtimes[0].pane_count, 0);
}

// ── Resize after pane exit ──────────────────────────────────────

#[tokio::test]
async fn resize_after_pane_exit_is_silently_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let runtime_id = create_and_attach(&mut client, "resize-exit").await;
    let pane_id = create_pane(&mut client, &runtime_id).await;

    // Kill the shell.
    send_input(&mut client, &runtime_id, &pane_id, b"exit\n").await;
    wait_for_pane_exited(&mut client, &pane_id).await;

    // Resize the dead pane — must be silently dropped, not panic or error.
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Resize(proto::Resize {
                runtime_id: runtime_id.clone(),
                pane_id,
                cols: 120,
                rows: 40,
            })),
        })
        .await;

    let msgs = client.drain(Duration::from_millis(200)).await;
    assert!(
        msgs.iter().all(|m| !matches!(m.msg, Some(proto::server_message::Msg::Error(_)))),
        "resize of exited pane must not produce an error response"
    );

    // Server must remain functional.
    let runtimes = list_runtimes(&mut client).await;
    assert_eq!(runtimes.len(), 1);
}

// ── Title change interleaved with output ────────────────────────

#[tokio::test]
async fn title_change_during_output_does_not_corrupt_state() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let runtime_id = create_and_attach(&mut client, "title-interleave").await;
    let pane_id = create_pane(&mut client, &runtime_id).await;

    // Send output that includes an OSC title-change sequence interleaved with data.
    let osc_title = b"\x1b]0;my-custom-title\x07";
    let mixed = [osc_title.as_slice(), b"echo after-title\n"].concat();
    send_input(&mut client, &runtime_id, &pane_id, &mixed).await;

    // Drain Deltas until we see the echo output.
    let target = b"after-title";
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for title-interleaved output");
        match client.try_recv(remaining).await {
            Some(msg) => {
                if let Some(proto::server_message::Msg::Delta(d)) = &msg.msg
                    && d.data.windows(target.len()).any(|w| w == target)
                {
                    break;
                }
            }
            None => panic!("timed out waiting for title-interleaved output"),
        }
    }

    // Server should still be responsive — list sessions as a liveness check.
    let runtimes = list_runtimes(&mut client).await;
    assert_eq!(runtimes.len(), 1);
    assert_eq!(runtimes[0].pane_count, 1);
}

// ── Shell exits before first attach ─────────────────────────────

#[tokio::test]
async fn shell_exits_before_reattach_shows_exit_status_in_inventory() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let runtime_id = create_and_attach(&mut client, "pre-attach-exit").await;
    let pane_id = create_pane(&mut client, &runtime_id).await;

    // Send exit, then detach before the shell finishes.
    send_input(&mut client, &runtime_id, &pane_id, b"exit\n").await;
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::DetachRuntime(proto::DetachRuntime {
                runtime_id: runtime_id.clone(),
            })),
        })
        .await;
    client.drain(Duration::from_millis(500)).await;

    // Poll inventory until the pane reports an exit status.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let runtimes = list_runtimes(&mut client).await;
        if !runtimes.is_empty()
            && !runtimes[0].panes.is_empty()
            && runtimes[0].panes[0].exit_status.is_some()
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for pane exit status in inventory"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Reattach — inventory should show exit status.
    let runtimes = list_runtimes(&mut client).await;
    assert_eq!(runtimes.len(), 1);
    assert_eq!(runtimes[0].panes.len(), 1);
    assert!(
        runtimes[0].panes[0].exit_status.is_some(),
        "pane that exited while detached must report exit status"
    );
}

// ── No duplicate PaneExited ─────────────────────────────────────

#[tokio::test]
async fn no_duplicate_pane_exited_after_close_of_exited_pane() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let runtime_id = create_and_attach(&mut client, "no-dup-exit").await;
    let pane_id = create_pane(&mut client, &runtime_id).await;

    // Exit the shell.
    send_input(&mut client, &runtime_id, &pane_id, b"exit\n").await;
    wait_for_pane_exited(&mut client, &pane_id).await;

    // Close the already-exited pane.
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
                runtime_id: runtime_id.clone(),
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
