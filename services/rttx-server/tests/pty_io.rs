//! Integration tests for PTY I/O: Delta streaming, Input routing, Resize.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::{bytes_to_uuid, proto};
use std::time::Duration;

/// Helper: create a session, create a pane, attach, and return IDs.
async fn setup_attached_pane(client: &mut TestClient) -> (Vec<u8>, Vec<u8>) {
    client.handshake().await;

    // Create session.
    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
            name: "io-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let session_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::SessionCreated(sc)) => sc.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    // Create pane (spawns PTY).
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

    // Attach to receive Deltas.
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

    (session_id, pane_id)
}

#[tokio::test]
async fn pane_creation_spawns_pty_and_produces_deltas() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let (session_id, pane_id) = setup_attached_pane(&mut client).await;

    // A shell produces a prompt or at least some output on startup.
    // Force output by sending a harmless command, then poll for the Delta.
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Input(proto::Input {
                session_id: session_id.clone(),
                pane_id: pane_id.clone(),
                data: bytes::Bytes::from_static(b"echo rttx_pty_test\n"),
            })),
        })
        .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for Delta from shell");
        match client.try_recv(remaining).await {
            Some(msg) if matches!(msg.msg, Some(proto::server_message::Msg::Delta(_))) => break,
            Some(_) => {}
            None => panic!("timed out waiting for Delta from shell"),
        }
    }
}

#[tokio::test]
async fn input_reaches_pty_and_echoes_back_as_delta() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let (session_id, pane_id) = setup_attached_pane(&mut client).await;

    // Drain initial shell startup output.
    client.drain(Duration::from_millis(500)).await;

    // Send input: a simple echo command.
    let marker = "RTTX_TEST_MARKER_42";
    let input_data = format!("echo {marker}\n");
    let input = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Input(proto::Input {
            session_id: session_id.clone(),
            pane_id: pane_id.clone(),
            data: bytes::Bytes::from(input_data.into_bytes()),
        })),
    };
    client.send(&input).await;

    // Collect output and look for the marker.
    let msgs = client.drain(Duration::from_secs(3)).await;
    let output: Vec<u8> = msgs
        .iter()
        .filter_map(|m| match &m.msg {
            Some(proto::server_message::Msg::Delta(d)) => Some(d.data.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    let output_str = String::from_utf8_lossy(&output);
    assert!(output_str.contains(marker), "expected '{marker}' in delta output, got: {output_str}");
}

#[tokio::test]
async fn resize_updates_pane_dimensions() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let (session_id, pane_id) = setup_attached_pane(&mut client).await;

    // Send resize.
    let resize = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Resize(proto::Resize {
            session_id: session_id.clone(),
            pane_id: pane_id.clone(),
            cols: 120,
            rows: 40,
        })),
    };
    client.send(&resize).await;
    assert!(matches!(
        client.recv_or_timeout().await.msg,
        Some(proto::server_message::Msg::PaneResized(_))
    ));

    // Verify by detaching and re-attaching: snapshot should show new dimensions.
    let detach = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::DetachSession(proto::DetachSession {
            session_id: session_id.clone(),
        })),
    };
    client.send(&detach).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for SessionDetached");
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::SessionDetached(_)) => break,
            Some(
                proto::server_message::Msg::Delta(_) | proto::server_message::Msg::PaneExited(_),
            ) => {}
            other => panic!("expected SessionDetached, got {other:?}"),
        }
    }

    // Small delay for the resize to process.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let attach = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
            session_id: session_id.clone(),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    let resp = client.recv_or_timeout().await;
    match resp.msg {
        Some(proto::server_message::Msg::Snapshot(snap)) => {
            let pane_snap =
                snap.panes.iter().find(|p| p.pane_id == pane_id).expect("pane not in snapshot");
            assert_eq!(pane_snap.cols, 120, "expected cols=120");
            assert_eq!(pane_snap.rows, 40, "expected rows=40");
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }
}

#[tokio::test]
async fn close_pane_kills_pty() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let (session_id, pane_id) = setup_attached_pane(&mut client).await;

    // Close the pane.
    let close = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
            session_id: session_id.clone(),
            pane_id: pane_id.clone(),
        })),
    };
    client.send(&close).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut saw_close = false;
    while tokio::time::Instant::now() < deadline {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::PaneClosed(_)) => {
                saw_close = true;
                break;
            }
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected PaneClosed, got {other:?}"),
        }
    }
    assert!(saw_close, "timed out waiting for PaneClosed");

    // Verify pane is gone: re-attach and check snapshot has no panes.
    let detach = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::DetachSession(proto::DetachSession {
            session_id: session_id.clone(),
        })),
    };
    client.send(&detach).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for SessionDetached");
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::SessionDetached(_)) => break,
            Some(
                proto::server_message::Msg::Delta(_) | proto::server_message::Msg::PaneExited(_),
            ) => {}
            other => panic!("expected SessionDetached, got {other:?}"),
        }
    }

    tokio::time::sleep(Duration::from_millis(100)).await;

    let attach = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
            session_id: session_id.clone(),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    let resp = client.recv_or_timeout().await;
    match resp.msg {
        Some(proto::server_message::Msg::Snapshot(snap)) => {
            assert!(snap.panes.is_empty(), "expected no panes after close, got: {:?}", snap.panes);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }
}

#[tokio::test]
async fn pane_exit_produces_pane_exited_message() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let (session_id, pane_id) = setup_attached_pane(&mut client).await;

    // Drain startup output.
    client.drain(Duration::from_millis(500)).await;

    // Tell the shell to exit with a specific code.
    let input = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Input(proto::Input {
            session_id: session_id.clone(),
            pane_id: pane_id.clone(),
            data: bytes::Bytes::from_static(b"exit 7\n"),
        })),
    };
    client.send(&input).await;

    // Collect messages and look for PaneExited.
    let msgs = client.drain(Duration::from_secs(3)).await;
    let exited = msgs.iter().find_map(|m| match &m.msg {
        Some(proto::server_message::Msg::PaneExited(pe)) => Some(pe.clone()),
        _ => None,
    });
    let exited = exited.expect("expected PaneExited message");

    let exitpane_id = bytes_to_uuid(&exited.pane_id).unwrap();
    let expectedpane_id = bytes_to_uuid(&pane_id).unwrap();
    assert_eq!(exitpane_id, expectedpane_id);
    assert_eq!(exited.status, 7);
}

#[tokio::test]
async fn ctrl_d_at_shell_prompt_produces_pane_exited_message() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let (session_id, pane_id) = setup_attached_pane(&mut client).await;
    client.drain(Duration::from_millis(500)).await;

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Input(proto::Input {
                session_id: session_id.clone(),
                pane_id: pane_id.clone(),
                data: bytes::Bytes::from_static(&[0x04]),
            })),
        })
        .await;

    let msgs = client.drain(Duration::from_secs(3)).await;
    let exited = msgs.iter().find_map(|m| match &m.msg {
        Some(proto::server_message::Msg::PaneExited(pe)) => Some(pe.clone()),
        _ => None,
    });

    let exited = exited.expect("Ctrl+D at shell prompt must produce PaneExited");
    assert_eq!(exited.pane_id, pane_id);
}

#[tokio::test]
async fn multi_client_delta_broadcast_delivers_identical_data() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client_a = TestClient::connect(&sock).await;
    let (session_id, pane_id) = setup_attached_pane(&mut client_a).await;
    client_a.drain(Duration::from_millis(500)).await;

    // Second client attaches read-only to the same session.
    let mut client_b = TestClient::connect(&sock).await;
    client_b.handshake().await;
    client_b
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadOnly as i32,
            })),
        })
        .await;
    loop {
        match client_b.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::Snapshot(_)) => break,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }
    }
    client_b.drain(Duration::from_millis(300)).await;

    // Send a command that produces deterministic output.
    let marker = "BYTES_BROADCAST_TEST_42";
    common::send_input(&mut client_a, &session_id, &pane_id, format!("echo {marker}\n").as_bytes())
        .await;

    // Collect Delta data from both clients.
    let collect = |msgs: &[proto::ServerMessage]| -> Vec<u8> {
        msgs.iter()
            .filter_map(|m| match &m.msg {
                Some(proto::server_message::Msg::Delta(d))
                    if bytes_to_uuid(&d.pane_id).ok() == bytes_to_uuid(&pane_id).ok() =>
                {
                    Some(d.data.to_vec())
                }
                _ => None,
            })
            .flatten()
            .collect()
    };

    let msgs_a = client_a.drain(Duration::from_secs(5)).await;
    let msgs_b = client_b.drain(Duration::from_secs(5)).await;

    let data_a = collect(&msgs_a);
    let data_b = collect(&msgs_b);

    let text_a = String::from_utf8_lossy(&data_a);
    let text_b = String::from_utf8_lossy(&data_b);

    assert!(text_a.contains(marker), "client A should receive the marker in Delta stream");
    assert!(text_b.contains(marker), "client B should receive the marker in Delta stream");
}
