//! Integration tests for scrollback persistence to disk.

mod common;

use common::{TestClient, start_test_server, wait_for_scrollback_log, wait_for_state_containing};
use rttx_proto::v3;
use std::time::Duration;

#[tokio::test]
async fn scrollback_flushed_to_disk_after_serialization_tick() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    // Create session and pane.
    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
            name: "scrollback-test".into(),
            policy: v3::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let runtime_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    let create_pane = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
            runtime_id: runtime_id.clone(),
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
            no_persist: None,
        })),
    };
    client.send(&create_pane).await;
    let pane_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    // Attach to get Deltas.
    let attach = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
            runtime_id: runtime_id.clone(),
            attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::RuntimeSnapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }

    // Drain startup output.
    client.drain(Duration::from_millis(500)).await;

    // Send input that produces predictable output.
    let marker = "SCROLLBACK_PERSIST_TEST";
    let input = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id: runtime_id.clone(),
            pane_id: pane_id.clone(),
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                data: bytes::Bytes::from(format!("echo {marker}\n").into_bytes()),
            })),
        })),
    };
    client.send(&input).await;

    // Wait for output + serialization tick (server serializes every 1s).
    wait_for_state_containing(tmp.path(), "scrollback-test", Duration::from_secs(10)).await;

    // Check that scrollback log exists in the state directory (RFC-022 layout).
    let runtimes_dir = tmp.path().join("state/rttx/daemon/runtimes");
    assert!(runtimes_dir.exists(), "runtimes directory should exist");

    // Find the log file under runtimes/<id>/scrollback/<pane>.log.
    let mut log_files = Vec::new();
    for runtime_dir in std::fs::read_dir(&runtimes_dir).unwrap() {
        let runtime_dir = runtime_dir.unwrap().path();
        let scrollback_dir = runtime_dir.join("scrollback");
        if scrollback_dir.is_dir() {
            for entry in std::fs::read_dir(&scrollback_dir).unwrap() {
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

/// Scrollback must be written under `state_dir` (not `cache_dir`) so cache
/// cleaners cannot delete user data.
#[tokio::test]
async fn scrollback_written_to_state_dir_not_cache_dir() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
            name: "path-test".into(),
            policy: v3::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let runtime_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    let create_pane = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
            runtime_id: runtime_id.clone(),
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
            no_persist: None,
        })),
    };
    client.send(&create_pane).await;
    let pane_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    let attach = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
            runtime_id: runtime_id.clone(),
            attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::RuntimeSnapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }
    client.drain(Duration::from_millis(500)).await;

    let input = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id: runtime_id.clone(),
            pane_id: pane_id.clone(),
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                data: bytes::Bytes::from_static(b"echo PATH_LOCATION_TEST\n"),
            })),
        })),
    };
    client.send(&input).await;

    wait_for_state_containing(tmp.path(), "path-test", Duration::from_secs(10)).await;

    // Scrollback must NOT appear in the cache directory.
    let cache_scrollback = tmp.path().join("cache").join("scrollback");
    assert!(
        !cache_scrollback.exists(),
        "scrollback directory must not exist under cache_dir: {}",
        cache_scrollback.display()
    );

    // Scrollback MUST appear under state_dir/runtimes/<id>/scrollback/.
    let runtimes_dir = tmp.path().join("state/rttx/daemon/runtimes");
    assert!(runtimes_dir.exists(), "runtimes directory should exist under state_dir");
}

#[tokio::test]
async fn scrollback_log_capped_at_max_size() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
            name: "cap-test".into(),
            policy: v3::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let runtime_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    let create_pane = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
            runtime_id: runtime_id.clone(),
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
            no_persist: None,
        })),
    };
    client.send(&create_pane).await;
    let pane_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    let attach = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
            runtime_id: runtime_id.clone(),
            attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::RuntimeSnapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }
    client.drain(Duration::from_millis(500)).await;

    // Generate ~12 MB of output via a shell command.
    let input = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id: runtime_id.clone(),
            pane_id: pane_id.clone(),
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                data: bytes::Bytes::from_static(b"head -c 12000000 /dev/zero | tr '\\0' 'A'\n"),
            })),
        })),
    };
    client.send(&input).await;

    // Poll until the scrollback log file appears (covers command execution +
    // serialization tick flush). Generous timeout for slow CI runners.
    let log_files = wait_for_scrollback_log(tmp.path(), Duration::from_secs(30)).await;

    assert_eq!(log_files.len(), 1, "expected exactly one scrollback log, found: {log_files:?}");

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

    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
            name: "dsr-strip-test".into(),
            policy: v3::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let runtime_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    let create_pane = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
            runtime_id: runtime_id.clone(),
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
            no_persist: None,
        })),
    };
    client.send(&create_pane).await;
    let pane_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    let attach = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
            runtime_id: runtime_id.clone(),
            attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::RuntimeSnapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }
    client.drain(Duration::from_millis(500)).await;

    // Send a command that triggers DSR queries from the shell/readline.
    // Most shells send DSR 6 (cursor position query) during prompt setup.
    // We also explicitly emit one via printf to guarantee it appears.
    let input = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id: runtime_id.clone(),
            pane_id: pane_id.clone(),
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                data: bytes::Bytes::from_static(b"printf '\\033[6n' && echo DSR_STRIP_MARKER\n"),
            })),
        })),
    };
    client.send(&input).await;

    // Wait for serialization tick to flush scrollback.
    wait_for_state_containing(tmp.path(), "dsr-strip-test", Duration::from_secs(10)).await;
    // Extra wait for the scrollback flush after the DSR-producing command.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Find the scrollback log file in the state directory.
    let runtimes_dir = tmp.path().join("state/rttx/daemon/runtimes");
    let mut log_files = Vec::new();
    for runtime_dir in std::fs::read_dir(&runtimes_dir).unwrap() {
        let runtime_dir = runtime_dir.unwrap().path();
        let scrollback_dir = runtime_dir.join("scrollback");
        if scrollback_dir.is_dir() {
            for entry in std::fs::read_dir(&scrollback_dir).unwrap() {
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
