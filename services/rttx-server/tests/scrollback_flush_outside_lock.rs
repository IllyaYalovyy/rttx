//! Integration test for out-of-lock scrollback flushing (#837).
//!
//! Verifies that scrollback data is correctly flushed to disk when the
//! flush I/O happens outside the server mutex (the drain-then-write
//! pattern introduced in #837).

mod common;

use common::{TestClient, start_test_server, wait_for_scrollback_log};
use rttx_proto::proto;
use std::time::Duration;

/// Scrollback is flushed to disk via the out-of-lock path after PTY output.
///
/// This exercises the serialization loop's drain-then-write pattern:
/// pending bytes are drained under the lock, then written to disk after
/// the lock is released.
#[tokio::test]
async fn scrollback_flushed_via_out_of_lock_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    // Create a persistent runtime with a pane.
    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "flush-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    })
    .await;
    let runtime_id = match c.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
            runtime_id: runtime_id.clone(),
            cwd: None,
            dark_background: None,
            cols: 80,
            rows: 24,
            no_persist: None,
        })),
    })
    .await;
    let pane_id = match c.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    // Attach to receive output.
    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
            runtime_id: runtime_id.clone(),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    })
    .await;
    match c.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::Snapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }
    c.drain(Duration::from_millis(500)).await;

    // Send a marker command so we can verify it appears in the scrollback log.
    let marker = "OUT_OF_LOCK_FLUSH_MARKER_837";
    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Input(proto::Input {
            runtime_id: runtime_id.clone(),
            pane_id: pane_id.clone(),
            data: bytes::Bytes::from(format!("echo {marker}\n").into_bytes()),
        })),
    })
    .await;

    // Wait for the serialization loop to flush scrollback to disk.
    let logs = wait_for_scrollback_log(tmp.path(), Duration::from_secs(10)).await;
    assert!(!logs.is_empty(), "at least one scrollback log should exist");

    // Verify the marker appears in the flushed scrollback.
    let mut found = false;
    for log_path in &logs {
        let content = std::fs::read(log_path).unwrap();
        let text = String::from_utf8_lossy(&content);
        if text.contains(marker) {
            found = true;
            break;
        }
    }
    assert!(found, "scrollback log should contain the marker after out-of-lock flush");
}
