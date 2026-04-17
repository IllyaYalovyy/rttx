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
            dark_background: None,
            cols: 0,
            rows: 0,
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
            data: bytes::Bytes::from(format!("echo {marker}\n").into_bytes()),
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

#[tokio::test]
async fn scrollback_log_capped_at_max_size() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
            name: "cap-test".into(),
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
            dark_background: None,
            cols: 0,
            rows: 0,
        })),
    };
    client.send(&create_pane).await;
    let pane_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

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
    client.drain(Duration::from_millis(500)).await;

    // Generate ~12 MB of output via a shell command.
    let input = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Input(proto::Input {
            session_id: session_id.clone(),
            pane_id: pane_id.clone(),
            data: bytes::Bytes::from_static(b"head -c 12000000 /dev/zero | tr '\\0' 'A'\n"),
        })),
    };
    client.send(&input).await;

    // Wait for command to finish + serialization ticks to flush and cap.
    tokio::time::sleep(Duration::from_secs(8)).await;

    // Find the scrollback log file.
    let scrollback_dir = tmp.path().join("cache").join("scrollback");
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

    assert!(!log_files.is_empty(), "expected at least one scrollback log");

    let size = std::fs::metadata(&log_files[0]).unwrap().len();
    let max = 10 * 1024 * 1024_u64; // 10 MB
    assert!(size <= max, "scrollback log is {size} bytes, exceeds {max} byte cap");
}

/// Scrollback logs must not contain DSR/DA1/DA2 query sequences.
/// If they did, replaying them after daemon restart would generate stale
/// CPR responses that appear as visible garbage (`;1R` fragments).
#[tokio::test]
async fn scrollback_log_does_not_contain_dsr_queries() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
            name: "dsr-strip-test".into(),
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
            dark_background: None,
            cols: 0,
            rows: 0,
        })),
    };
    client.send(&create_pane).await;
    let pane_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

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
    client.drain(Duration::from_millis(500)).await;

    // Send a command that triggers DSR queries from the shell/readline.
    // Most shells send DSR 6 (cursor position query) during prompt setup.
    // We also explicitly emit one via printf to guarantee it appears.
    let input = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Input(proto::Input {
            session_id: session_id.clone(),
            pane_id: pane_id.clone(),
            data: bytes::Bytes::from_static(b"printf '\\033[6n' && echo DSR_STRIP_MARKER\n"),
        })),
    };
    client.send(&input).await;

    // Wait for serialization tick to flush scrollback.
    wait_for_state_containing(&tmp.path().join("cache"), "dsr-strip-test", Duration::from_secs(10))
        .await;
    // Extra wait for the scrollback flush after the DSR-producing command.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Find the scrollback log file.
    let scrollback_dir = tmp.path().join("cache").join("scrollback");
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
    assert!(!log_files.is_empty(), "expected at least one scrollback log");

    let content = std::fs::read(&log_files[0]).unwrap();

    // The log must contain our marker.
    let text = String::from_utf8_lossy(&content);
    assert!(text.contains("DSR_STRIP_MARKER"), "scrollback should contain marker, got: {text}");

    // The log must NOT contain DSR query sequences (ESC[6n, ESC[5n, ESC[c, ESC[>c).
    let dsr_patterns: &[&[u8]] = &[
        b"\x1b[6n", // DSR cursor position
        b"\x1b[5n", // DSR operating status
    ];
    for pattern in dsr_patterns {
        assert!(
            !contains_bytes(&content, pattern),
            "scrollback log should not contain DSR query {:?}",
            String::from_utf8_lossy(pattern),
        );
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
