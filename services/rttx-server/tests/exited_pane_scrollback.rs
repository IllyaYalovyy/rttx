//! Regression test: exited panes release their scrollback buffer.
//!
//! Verifies that after a pane's process exits, the server releases the
//! in-memory scrollback so reconnecting clients receive an empty snapshot
//! for that pane. Prevents unbounded RSS growth from accumulated exited
//! panes (#541).

mod common;

use common::*;
use rttx_proto::proto;
use std::time::Duration;

#[tokio::test]
async fn exited_pane_snapshot_has_empty_scrollback() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let sid = create_session(&mut client, "exit-test", proto::RuntimePolicy::Persistent).await;
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
    detach_session(&mut client, &sid).await;
    let snapshot = attach_rw(&mut client, &sid).await;

    // The exited pane should have empty scrollback.
    let exited_pane = snapshot
        .panes
        .iter()
        .find(|p| p.pane_id == pane_id)
        .expect("exited pane should still be in snapshot");
    assert!(
        exited_pane.scrollback.is_empty(),
        "exited pane scrollback should be empty, got {} bytes",
        exited_pane.scrollback.len()
    );
    assert!(exited_pane.exit_status.is_some(), "pane should have an exit status");
}
