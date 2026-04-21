//! Tests that simulate the GUI's restore flow using `DaemonBridge`.
//!
//! These verify the exact sequence the GUI performs:
//! 1. connect + `list_runtimes`
//! 2. `attach_runtime` for each → get snapshot with scrollback
//! 3. disconnect
//! 4. reconnect + `list_runtimes` → same count, same IDs

mod common;

use common::{TestClient, start_test_server, wait_for_state_containing};
use rttx_proto::proto;
use std::time::Duration;

/// Simulate the GUI restore flow: connect, list, attach each session,
/// verify snapshots have scrollback, disconnect, reconnect, verify
/// session count is unchanged.
#[tokio::test]
async fn gui_restore_flow_no_duplicates() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    // --- Setup: create 2 sessions with output ---
    let mut session_ids = Vec::new();
    {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        for i in 1..=2 {
            c.send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
                    name: format!("Session {i}"),
                    policy: proto::RuntimePolicy::Persistent as i32,
                })),
            })
            .await;
            let sid = match c.recv().await.msg {
                Some(proto::server_message::Msg::RuntimeCreated(sc)) => sc.runtime_id,
                other => panic!("expected RuntimeCreated, got {other:?}"),
            };

            c.send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                    runtime_id: sid.clone(),
                    cwd: None,
                    dark_background: None,
                    cols: 0,
                    rows: 0,
                    no_persist: None,
                })),
            })
            .await;
            // Drain any interleaved deltas to find PaneCreated.
            let pid = loop {
                match c.recv().await.msg {
                    Some(proto::server_message::Msg::PaneCreated(pc)) => break pc.pane_id,
                    Some(proto::server_message::Msg::Delta(_)) => {}
                    other => panic!("expected PaneCreated or Delta, got {other:?}"),
                }
            };

            c.send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
                    runtime_id: sid.clone(),
                    attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
                })),
            })
            .await;
            // Drain deltas to find Snapshot.
            loop {
                match c.recv().await.msg {
                    Some(proto::server_message::Msg::Snapshot(_)) => break,
                    Some(proto::server_message::Msg::Delta(_)) => {}
                    other => panic!("expected Snapshot or Delta, got {other:?}"),
                }
            }

            c.send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::Input(proto::Input {
                    runtime_id: sid.clone(),
                    pane_id: pid,
                    data: bytes::Bytes::from(format!("echo MARKER_{i}\n").into_bytes()),
                })),
            })
            .await;

            session_ids.push(sid);
        }

        // Wait for output.
        wait_for_state_containing(&tmp.path().join("cache"), "Session", Duration::from_secs(10))
            .await;
        let _ = c.drain(Duration::from_millis(500)).await;
    }

    // --- Simulate GUI restore: connect, list, attach each ---
    {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ListRuntimes(proto::ListRuntimes {})),
        })
        .await;
        let runtimes = match c.recv().await.msg {
            Some(proto::server_message::Msg::RuntimeList(sl)) => sl.runtimes,
            other => panic!("expected RuntimeList, got {other:?}"),
        };
        assert_eq!(runtimes.len(), 2, "should have exactly 2 sessions");

        // Sort by name for deterministic comparison.
        let mut sorted_runtimes = runtimes.clone();
        sorted_runtimes.sort_by(|a, b| a.name.cmp(&b.name));
        let sorted_ids = session_ids.clone();
        // session_ids[0] is "Session 1", session_ids[1] is "Session 2" — already sorted.

        for (i, info) in sorted_runtimes.iter().enumerate() {
            assert_eq!(info.id, sorted_ids[i]);

            c.send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
                    runtime_id: info.id.clone(),
                    attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
                })),
            })
            .await;
            let snapshot = loop {
                match c.recv().await.msg {
                    Some(proto::server_message::Msg::Snapshot(s)) => break s,
                    Some(proto::server_message::Msg::Delta(_)) => {}
                    other => panic!("expected Snapshot or Delta, got {other:?}"),
                }
            };
            assert!(!snapshot.panes.is_empty(), "session {i} should have panes");

            let scrollback = String::from_utf8_lossy(&snapshot.panes[0].scrollback);
            let marker = format!("MARKER_{}", i + 1);
            assert!(scrollback.contains(&marker), "session {i} scrollback should contain {marker}");
        }
    }

    // --- Simulate second GUI restore: same count, no new sessions ---
    {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ListRuntimes(proto::ListRuntimes {})),
        })
        .await;
        let runtimes = match c.recv().await.msg {
            Some(proto::server_message::Msg::RuntimeList(sl)) => sl.runtimes,
            other => panic!("expected RuntimeList, got {other:?}"),
        };
        assert_eq!(
            runtimes.len(),
            2,
            "second restore should still see exactly 2 sessions, not more"
        );
    }
}
