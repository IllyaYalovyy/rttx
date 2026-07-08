//! Integration test: a daemon restart persists terminal-mode metadata in the
//! screen snapshot, but reconstructs the pane to a clean interactive baseline
//! rather than restoring a dead TUI's interaction modes (mouse tracking,
//! alt-screen, …) onto the freshly respawned shell.

mod common;

use common::{
    attach_rw, create_pane, create_workspace, send_input, start_test_server,
    wait_for_state_containing,
};
use rttx_proto::{bytes_to_uuid, v3};
use rttx_server::state::persistence;
use std::time::Duration;

/// The daemon persists the pane's terminal modes in the on-disk snapshot, but
/// after a restart the process that set those modes is gone. Reconstruction
/// must reset interaction modes (mouse tracking, SGR mouse, …) to a clean
/// baseline instead of restoring them — otherwise the respawned shell inherits
/// stuck mouse tracking and pointer movement injects `\x1b[<btn;col;rowM`
/// reports onto the prompt (the reported "daemon spitting garbage" bug).
#[tokio::test]
async fn restart_resets_tui_modes_instead_of_restoring_them() {
    let tmp = tempfile::TempDir::new().unwrap();

    let runtime_id_bytes;
    let pane_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut c = common::TestClient::connect(&sock).await;
        c.handshake().await;

        runtime_id_bytes =
            create_workspace(&mut c, "mode-restore", v3::WorkspacePolicy::Persistent).await;
        let pane_id_bytes = create_pane(&mut c, &runtime_id_bytes).await;
        pane_id = bytes_to_uuid(&pane_id_bytes).unwrap();
        attach_rw(&mut c, &runtime_id_bytes).await;

        // Enable several terminal modes via escape sequences.
        send_input(
            &mut c,
            &runtime_id_bytes,
            &pane_id_bytes,
            b"printf '\\033[?2004h\\033[?1h\\033[?1003h\\033[?1006h\\033[?1004h\\033[?25l'\n",
        )
        .await;

        // Wait for serialization tick to persist the snapshot.
        wait_for_state_containing(tmp.path(), "mode-restore", Duration::from_secs(10)).await;

        // Drain output.
        let _ = tokio::time::timeout(Duration::from_millis(500), c.recv()).await;

        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Verify the persisted snapshot has the expected modes.
    let state_dir = tmp.path().join("state/rttx/daemon");
    let runtime_id = bytes_to_uuid(&runtime_id_bytes).unwrap();
    let snap = persistence::load_screen_snapshot(&state_dir, runtime_id, pane_id)
        .expect("snapshot should exist after serialization");

    assert!(snap.modes.bracketed_paste, "bracketed paste should be persisted");
    assert!(snap.modes.application_cursor_keys, "app cursor keys should be persisted");
    assert_eq!(snap.modes.mouse_tracking_mode, 1003, "mouse tracking should be persisted");
    assert!(snap.modes.sgr_mouse, "SGR mouse should be persisted");
    assert!(snap.modes.focus_reporting, "focus reporting should be persisted");
    assert!(!snap.cursor_visible, "cursor should be hidden in snapshot");

    // Phase 2: restart and verify modes are restored.
    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut c = common::TestClient::connect(&sock).await;
        c.handshake().await;

        let snapshot = attach_rw(&mut c, &runtime_id_bytes).await;
        let pane = snapshot
            .panes
            .iter()
            .find(|p| bytes_to_uuid(&p.pane_id).unwrap() == pane_id)
            .expect("pane should be present after restart");

        // The pane should have a live shell (not exited).
        assert!(pane.exit_status.is_none(), "reconstructed pane should have a live shell");

        // The dead TUI's interaction modes must NOT be restored onto the fresh
        // shell. A bare shell never enables mouse tracking, so a reset mouse
        // mode proves reconstruction rebuilt a clean baseline rather than
        // replaying the app's stuck-mouse state.
        let modes = pane.terminal_modes.as_ref().expect("terminal modes present");
        assert_eq!(
            modes.mouse_mode,
            v3::MouseMode::None as i32,
            "mouse tracking must be reset after restart, not restored",
        );
        assert!(!modes.sgr_mouse, "SGR mouse must be reset after restart");
    }
}

/// A normal shell (no alt-screen, no mouse tracking) keeps its scrollback across
/// a daemon restart. The transient-frame skip that fixes the TUI garbage bug
/// must not discard genuine shell history.
#[tokio::test]
async fn restart_preserves_normal_shell_scrollback() {
    const MARKER: &str = "MARKER_ABC123";

    let tmp = tempfile::TempDir::new().unwrap();
    let state_dir = tmp.path().join("state/rttx/daemon");

    let runtime_id_bytes;
    let pane_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut c = common::TestClient::connect(&sock).await;
        c.handshake().await;

        runtime_id_bytes =
            create_workspace(&mut c, "scrollback-keep", v3::WorkspacePolicy::Persistent).await;
        let pane_id_bytes = create_pane(&mut c, &runtime_id_bytes).await;
        pane_id = bytes_to_uuid(&pane_id_bytes).unwrap();
        attach_rw(&mut c, &runtime_id_bytes).await;

        // Emit a unique marker as ordinary shell output — no TUI modes involved.
        send_input(&mut c, &runtime_id_bytes, &pane_id_bytes, b"printf 'MARKER_ABC123\\n'\n").await;

        // Wait until the marker is captured in the persisted screen snapshot so
        // the restart below has it on disk to replay.
        let runtime_id = bytes_to_uuid(&runtime_id_bytes).unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(snap) = persistence::load_screen_snapshot(&state_dir, runtime_id, pane_id)
                && snap.screen_bytes.windows(MARKER.len()).any(|w| w == MARKER.as_bytes())
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "marker never persisted to screen snapshot",
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Phase 2: restart and confirm the normal-shell scrollback replays.
    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut c = common::TestClient::connect(&sock).await;
        c.handshake().await;

        let snapshot = attach_rw(&mut c, &runtime_id_bytes).await;
        let pane = snapshot
            .panes
            .iter()
            .find(|p| bytes_to_uuid(&p.pane_id).unwrap() == pane_id)
            .expect("pane should be present after restart");

        let tail = String::from_utf8_lossy(&pane.scrollback_tail);
        assert!(
            tail.contains(MARKER),
            "normal-shell scrollback should survive restart, got: {tail:?}",
        );
    }
}
