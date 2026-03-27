//! Test that simulates exactly what the GUI's make_pane_persistent does:
//! connect → create session → attach → create pane → send input → read delta.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::{bytes_to_uuid, proto, uuid_to_bytes};
use std::time::Duration;

/// Simulate the exact sequence make_pane_persistent_impl performs.
#[tokio::test]
async fn make_pane_persistent_flow() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    // 1. Create session.
    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
            name: "pane-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    })
    .await;
    let session_id = match c.recv().await.msg {
        Some(proto::server_message::Msg::SessionCreated(sc)) => sc.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    };
    let session_uuid = bytes_to_uuid(&session_id).unwrap();

    // 2. Attach session.
    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
            session_id: session_id.clone(),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    })
    .await;
    let snapshot = loop {
        match c.recv().await.msg {
            Some(proto::server_message::Msg::Snapshot(s)) => break s,
            Some(proto::server_message::Msg::Delta(_)) => continue,
            other => panic!("expected Snapshot, got {other:?}"),
        }
    };
    assert!(snapshot.panes.is_empty(), "new session should have no panes yet");

    // 3. Create pane.
    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
            session_id: session_id.clone(),
        })),
    })
    .await;
    let pane_id = loop {
        match c.recv().await.msg {
            Some(proto::server_message::Msg::PaneCreated(pc)) => break pc.pane_id,
            Some(proto::server_message::Msg::Delta(_)) => continue,
            other => panic!("expected PaneCreated, got {other:?}"),
        }
    };
    let pane_uuid = bytes_to_uuid(&pane_id).unwrap();

    // 4. Send input (cd to a directory).
    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Input(proto::Input {
            session_id: session_id.clone(),
            pane_id: pane_id.clone(),
            data: b"echo PERSIST_OK\n".to_vec(),
        })),
    })
    .await;

    // 5. Read deltas until we see our marker.
    let mut found_marker = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), c.recv()).await {
            Ok(msg) => match msg.msg {
                Some(proto::server_message::Msg::Delta(d)) => {
                    let text = String::from_utf8_lossy(&d.data);
                    if text.contains("PERSIST_OK") {
                        found_marker = true;
                        break;
                    }
                }
                _ => {}
            },
            Err(_) => break,
        }
    }
    assert!(found_marker, "should receive delta with our echo output");

    // 6. Verify we can send resize.
    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Resize(proto::Resize {
            session_id: session_id.clone(),
            pane_id: pane_id.clone(),
            cols: 120,
            rows: 40,
        })),
    })
    .await;
    assert!(matches!(c.recv().await.msg, Some(proto::server_message::Msg::PaneResized(_))));

    // 7. Disconnect and reconnect — verify session persists.
    drop(c);

    let mut c2 = TestClient::connect(&sock).await;
    c2.handshake().await;

    c2.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
    })
    .await;
    let sessions = match c2.recv().await.msg {
        Some(proto::server_message::Msg::SessionList(sl)) => sl.sessions,
        other => panic!("expected SessionList, got {other:?}"),
    };
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].pane_count, 1);

    // 8. Re-attach and verify scrollback.
    c2.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
            session_id: session_id.clone(),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    })
    .await;
    let snapshot = loop {
        match c2.recv().await.msg {
            Some(proto::server_message::Msg::Snapshot(s)) => break s,
            Some(proto::server_message::Msg::Delta(_)) => continue,
            other => panic!("expected Snapshot, got {other:?}"),
        }
    };
    assert_eq!(snapshot.panes.len(), 1);
    let scrollback = String::from_utf8_lossy(&snapshot.panes[0].scrollback);
    assert!(
        scrollback.contains("PERSIST_OK"),
        "scrollback should contain our marker after reconnect"
    );
}
