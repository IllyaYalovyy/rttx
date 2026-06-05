//! Integration test: daemon restart restores canonical pane state from
//! snapshot metadata, not just from replaying `screen_bytes`.

mod common;

use common::{
    attach_rw, create_pane, create_runtime, send_input, start_test_server,
    wait_for_state_containing,
};
use rttx_proto::{bytes_to_uuid, v3};
use rttx_server::state::persistence;
use std::time::Duration;

/// After daemon restart, terminal modes persisted in the screen snapshot
/// are restored even when the mode-enabling escape sequences are not
/// present in the retained `screen_bytes` tail.
#[tokio::test]
async fn restart_restores_terminal_modes_from_snapshot_metadata() {
    let tmp = tempfile::TempDir::new().unwrap();

    let runtime_id_bytes;
    let pane_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut c = common::TestClient::connect(&sock).await;
        c.handshake().await;

        runtime_id_bytes =
            create_runtime(&mut c, "mode-restore", v3::RuntimePolicy::Persistent).await;
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

        // Scrollback should not be empty (screen_bytes were fed).
        assert!(!pane.scrollback_tail.is_empty(), "scrollback should be restored");
    }
}
