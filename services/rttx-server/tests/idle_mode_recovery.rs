//! A pane whose full-screen app died without cleaning up must not hand a
//! reattaching client a terminal that needs `reset`.
//!
//! The shell is the foreground process again, so mouse tracking, the
//! alternate screen and a hidden cursor can only be leftovers; the daemon
//! clears them at attach and renders a clean snapshot.

mod common;

use common::*;
use rttx_proto::v3;
use std::time::Duration;

#[tokio::test]
async fn modes_left_by_a_dead_app_are_cleared_when_a_client_attaches() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;
    let sid = create_workspace(&mut client, "dead-app", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &sid).await;
    let pane_id = create_pane(&mut client, &sid).await;

    // What a crashed TUI leaves on the terminal: alternate screen, mouse
    // tracking with SGR encoding, focus reporting, and a hidden cursor —
    // and then the shell prompt comes back with all of it still armed.
    send_input(
        &mut client,
        &sid,
        &pane_id,
        b"printf '\\033[?1049h\\033[?1000h\\033[?1006h\\033[?1004h\\033[?25l'\n",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(800)).await;

    detach_workspace(&mut client, &sid).await;
    let snapshot = attach_rw(&mut client, &sid).await;
    let pane = snapshot.panes.iter().find(|p| p.pane_id == pane_id).expect("pane in snapshot");

    let modes = pane.terminal_modes.as_ref().expect("terminal modes in snapshot");
    assert_eq!(modes.mouse_mode, v3::MouseMode::None as i32, "mouse tracking must be cleared");
    assert!(!modes.sgr_mouse, "SGR mouse encoding must be cleared");
    assert!(!modes.alternate_screen, "alternate screen must be left");
    assert!(!modes.focus_reporting, "focus reporting must be cleared");

    let stream = &pane.scrollback_tail;
    for armed in [
        &b"\x1b[?1000h"[..],
        b"\x1b[?1002h",
        b"\x1b[?1003h",
        b"\x1b[?1006h",
        b"\x1b[?1049h",
        b"\x1b[?1004h",
        b"\x1b[?25l",
    ] {
        assert!(
            !stream.windows(armed.len()).any(|w| w == armed),
            "the attach stream must not arm {:?}: {:?}",
            String::from_utf8_lossy(armed),
            String::from_utf8_lossy(stream)
        );
    }

    // The client can type: the prompt is there and the next command runs.
    send_input(&mut client, &sid, &pane_id, b"echo STILL_USABLE_42\n").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut seen = Vec::new();
    while tokio::time::Instant::now() < deadline {
        if let Some(msg) = client.try_recv(Duration::from_millis(200)).await
            && let Some(v3::server_envelope::Payload::OutputDelta(delta)) = msg.payload
        {
            seen.extend_from_slice(&delta.data);
            if seen.windows(15).any(|w| w == b"STILL_USABLE_42") {
                break;
            }
        }
    }
    assert!(seen.windows(15).any(|w| w == b"STILL_USABLE_42"), "shell must still execute commands");
}
