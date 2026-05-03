//! Integration test: scrollback stores pre-stripped data (#831).
//!
//! Verifies that `accept_output` strips terminal query sequences before
//! storing bytes in `pending_flush`, so `write_scrollback_to_disk` writes
//! clean data without a redundant second strip pass.

mod common;

use common::{TestClient, start_test_server, wait_for_scrollback_log};
use rttx_proto::proto;
use std::time::Duration;

/// Scrollback log must not contain DSR queries even though stripping
/// now happens at accept time rather than at flush time.
#[tokio::test]
async fn scrollback_pre_stripped_at_accept_time() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "pre-strip-test".into(),
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
            cols: 0,
            rows: 0,
            no_persist: None,
        })),
    })
    .await;
    let pane_id = match c.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

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

    // Emit DSR + DA1 queries interleaved with a marker.
    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Input(proto::Input {
            runtime_id: runtime_id.clone(),
            pane_id: pane_id.clone(),
            data: bytes::Bytes::from_static(
                b"printf 'PRE_STRIP\\033[6n\\033[cMARKER'\n",
            ),
        })),
    })
    .await;

    let logs = wait_for_scrollback_log(tmp.path(), Duration::from_secs(15)).await;
    assert!(!logs.is_empty(), "scrollback log should exist");

    let content = std::fs::read(&logs[0]).unwrap();
    let text = String::from_utf8_lossy(&content);
    assert!(text.contains("PRE_STRIP"), "marker should be in scrollback: {text}");

    // DSR and DA1 queries must have been stripped at accept time.
    assert!(
        !content.windows(4).any(|w| w == b"\x1b[6n"),
        "DSR query must not appear in scrollback log",
    );
    assert!(
        !content.windows(3).any(|w| w == b"\x1b[c"),
        "DA1 query must not appear in scrollback log",
    );
}
