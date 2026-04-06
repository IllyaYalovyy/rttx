//! Integration tests for scrollback persistence to disk.

mod common;

use common::{TestClient, start_test_server, wait_for_state_containing};
use rttx_proto::proto;
use std::time::Duration;

#[tokio::test]
async fn scrollback_flushed_to_disk_after_serialization_tick() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    // Create session and pane.
    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
            name: "scrollback-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let session_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::SessionCreated(sc)) => sc.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    let create_pane = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
            session_id: session_id.clone(),
            cwd: None,
        })),
    };
    client.send(&create_pane).await;
    let pane_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    // Attach to get Deltas.
    let attach = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
            session_id: session_id.clone(),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::Snapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }

    // Drain startup output.
    client.drain(Duration::from_millis(500)).await;

    // Send input that produces predictable output.
    let marker = "SCROLLBACK_PERSIST_TEST";
    let input = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Input(proto::Input {
            session_id: session_id.clone(),
            pane_id: pane_id.clone(),
            data: format!("echo {marker}\n").into_bytes(),
        })),
    };
    client.send(&input).await;

    // Wait for output + serialization tick (server serializes every 1s).
    wait_for_state_containing(
        &tmp.path().join("cache"),
        "scrollback-test",
        Duration::from_secs(10),
    )
    .await;

    // Check that scrollback log exists in the cache directory.
    let scrollback_dir = tmp.path().join("cache").join("scrollback");
    assert!(scrollback_dir.exists(), "scrollback directory should exist");

    // Find the log file (we don't know the exact UUIDs, but there should be exactly one).
    let mut log_files = Vec::new();
    for session_dir in std::fs::read_dir(&scrollback_dir).unwrap() {
        let session_dir = session_dir.unwrap().path();
        if session_dir.is_dir() {
            for entry in std::fs::read_dir(&session_dir).unwrap() {
                let entry = entry.unwrap().path();
                if entry.extension().is_some_and(|ext| ext == "log") {
                    log_files.push(entry);
                }
            }
        }
    }

    assert_eq!(log_files.len(), 1, "expected exactly one scrollback log, found: {log_files:?}");

    let content = std::fs::read_to_string(&log_files[0]).unwrap();
    assert!(content.contains(marker), "expected '{marker}' in scrollback log, got: {content}");
}
