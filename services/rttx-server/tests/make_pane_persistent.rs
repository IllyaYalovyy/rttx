//! Test that simulates exactly what the GUI's `make_pane_persistent` does:
//! connect → create session → attach → create pane → send input → read delta.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::{bytes_to_uuid, v3};
use std::time::Duration;

/// Simulate the exact sequence `make_pane_persistent_impl` performs.
#[tokio::test]
async fn make_pane_persistent_flow() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    // 1. Create session.
    c.send(&v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
            name: "pane-test".into(),
            policy: v3::RuntimePolicy::Persistent as i32,
        })),
    })
    .await;
    let runtime_id = match c.recv().await.payload {
        Some(v3::server_envelope::Payload::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };
    let _session_uuid = bytes_to_uuid(&runtime_id).unwrap();

    // 2. Attach session.
    c.send(&v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
            runtime_id: runtime_id.clone(),
            attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
        })),
    })
    .await;
    let snapshot = loop {
        match c.recv().await.payload {
            Some(v3::server_envelope::Payload::RuntimeSnapshot(s)) => break s,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }
    };
    assert!(snapshot.panes.is_empty(), "new session should have no panes yet");

    // 3. Create pane.
    c.send(&v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
            runtime_id: runtime_id.clone(),
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
            no_persist: None,
        })),
    })
    .await;
    let pane_id = loop {
        match c.recv().await.payload {
            Some(v3::server_envelope::Payload::PaneCreated(pc)) => break pc.pane_id,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected PaneCreated, got {other:?}"),
        }
    };
    let _pane_uuid = bytes_to_uuid(&pane_id).unwrap();

    // 4. Send input (cd to a directory).
    c.send(&v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id: runtime_id.clone(),
            pane_id: pane_id.clone(),
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                data: bytes::Bytes::from_static(b"echo PERSIST_OK\n"),
            })),
        })),
    })
    .await;

    // 5. Read deltas until we see our marker.
    let mut found_marker = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), c.recv()).await {
            Ok(msg) => {
                if let Some(v3::server_envelope::Payload::OutputDelta(d)) = msg.payload {
                    let text = String::from_utf8_lossy(&d.data);
                    if text.contains("PERSIST_OK") {
                        found_marker = true;
                        break;
                    }
                }
            }
            Err(_) => break,
        }
    }
    assert!(found_marker, "should receive delta with our echo output");

    // 6. Verify we can send resize.
    c.send(&v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::ResizePane(v3::ResizePane {
            runtime_id: runtime_id.clone(),
            pane_id: pane_id.clone(),
            cols: 120,
            rows: 40,
        })),
    })
    .await;
    assert!(matches!(c.recv().await.payload, Some(v3::server_envelope::Payload::PaneResized(_))));

    // 7. Disconnect and reconnect — verify session persists.
    drop(c);

    let mut c2 = TestClient::connect(&sock).await;
    c2.handshake().await;

    c2.send(&v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::ListRuntimes(v3::ListRuntimes {})),
    })
    .await;
    let runtimes = match c2.recv().await.payload {
        Some(v3::server_envelope::Payload::RuntimeList(sl)) => sl.runtimes,
        other => panic!("expected RuntimeList, got {other:?}"),
    };
    assert_eq!(runtimes.len(), 1);
    assert_eq!(runtimes[0].pane_count, 1);

    // 8. Re-attach and verify scrollback.
    c2.send(&v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
            runtime_id: runtime_id.clone(),
            attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
        })),
    })
    .await;
    let snapshot = loop {
        match c2.recv().await.payload {
            Some(v3::server_envelope::Payload::RuntimeSnapshot(s)) => break s,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }
    };
    assert_eq!(snapshot.panes.len(), 1);
    let scrollback = String::from_utf8_lossy(&snapshot.panes[0].scrollback_tail);
    assert!(
        scrollback.contains("PERSIST_OK"),
        "scrollback should contain our marker after reconnect"
    );
}
