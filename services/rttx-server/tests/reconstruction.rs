//! Integration tests for session reconstruction after daemon restart.

mod common;

use common::{TestClient, start_test_server, wait_for_state_containing};
use rttx_proto::proto;
use std::time::Duration;

/// Create a session with a pane, write to it, stop the server, restart,
/// and verify the scrollback is restored in the snapshot.
#[tokio::test]
async fn reconstruct_session_after_restart() {
    let tmp = tempfile::TempDir::new().unwrap();

    // Phase 1: start server, create session, produce output, let serialization tick.
    let session_id;
    let pane_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        // Create session.
        let create = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: "reconstruct-test".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        };
        client.send(&create).await;
        let resp = client.recv().await;
        session_id = match resp.msg {
            Some(proto::server_message::Msg::SessionCreated(sc)) => sc.session_id,
            other => panic!("expected SessionCreated, got {other:?}"),
        };

        // Create pane (spawns a PTY).
        let create_pane = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                session_id: session_id.clone(),
            })),
        };
        client.send(&create_pane).await;
        let resp = client.recv().await;
        pane_id = match resp.msg {
            Some(proto::server_message::Msg::PaneCreated(pc)) => pc.pane_id,
            other => panic!("expected PaneCreated, got {other:?}"),
        };

        // Attach to get deltas.
        let attach = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        };
        client.send(&attach).await;
        let _snapshot = client.recv().await;

        // Send a command that produces recognizable output.
        let input = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Input(proto::Input {
                session_id: session_id.clone(),
                pane_id: pane_id.clone(),
                data: b"echo RECONSTRUCT_MARKER\n".to_vec(),
            })),
        };
        client.send(&input).await;

        // Wait for output and serialization tick (>1s).
        wait_for_state_containing(
            &tmp.path().join("cache"),
            "reconstruct-test",
            Duration::from_secs(10),
        )
        .await;

        // Drain any pending deltas.
        let _ = tokio::time::timeout(Duration::from_millis(200), client.recv()).await;

        // Kill the server (simulates crash).
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Phase 2: restart server, verify reconstruction.
    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        // List sessions — should find our session.
        let list = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
        };
        client.send(&list).await;
        let resp = client.recv().await;
        let sessions = match resp.msg {
            Some(proto::server_message::Msg::SessionList(sl)) => sl.sessions,
            other => panic!("expected SessionList, got {other:?}"),
        };
        assert_eq!(sessions.len(), 1, "session should be restored");
        assert_eq!(sessions[0].name, "reconstruct-test");
        assert_eq!(sessions[0].id, session_id);

        // Attach and check snapshot contains scrollback with our marker.
        let attach = proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        };
        client.send(&attach).await;
        let resp = client.recv().await;
        let panes = match resp.msg {
            Some(proto::server_message::Msg::Snapshot(snap)) => snap.panes,
            other => panic!("expected Snapshot, got {other:?}"),
        };
        assert!(!panes.is_empty(), "should have at least one pane");

        let scrollback = String::from_utf8_lossy(&panes[0].scrollback);
        assert!(
            scrollback.contains("RECONSTRUCT_MARKER"),
            "scrollback should contain our marker after reconstruction, got: {scrollback}"
        );

        // The pane should not be exited (fresh shell was spawned).
        assert!(panes[0].exit_status.is_none(), "reconstructed pane should have a live shell");
    }
}

#[tokio::test]
async fn reconstruct_session_respawns_shell_in_last_reported_cwd() {
    let tmp = tempfile::TempDir::new().unwrap();
    let project_dir = tmp.path().join("project");
    std::fs::create_dir_all(&project_dir).unwrap();
    let project_dir_string = project_dir.to_string_lossy().to_string();

    let session_id;
    let pane_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                    name: "reconstruct-cwd".into(),
                    policy: proto::RuntimePolicy::Persistent as i32,
                })),
            })
            .await;
        session_id = match client.recv().await.msg {
            Some(proto::server_message::Msg::SessionCreated(created)) => created.session_id,
            other => panic!("expected SessionCreated, got {other:?}"),
        };

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                    session_id: session_id.clone(),
                })),
            })
            .await;
        pane_id = match client.recv().await.msg {
            Some(proto::server_message::Msg::PaneCreated(created)) => created.pane_id,
            other => panic!("expected PaneCreated, got {other:?}"),
        };

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                    session_id: session_id.clone(),
                    attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
                })),
            })
            .await;
        match client.recv().await.msg {
            Some(proto::server_message::Msg::Snapshot(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }

        let cwd_command = format!(
            "cd '{}'\nprintf '\\033]7;file://localhost%s\\007' \"$PWD\"\n",
            shell_quote(&project_dir_string)
        );
        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::Input(proto::Input {
                    session_id: session_id.clone(),
                    pane_id: pane_id.clone(),
                    data: cwd_command.into_bytes(),
                })),
            })
            .await;

        wait_for_state_containing(
            &tmp.path().join("cache"),
            "reconstruct-cwd",
            Duration::from_secs(10),
        )
        .await;
        let _ = tokio::time::timeout(Duration::from_millis(200), client.recv()).await;

        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                    session_id: session_id.clone(),
                    attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
                })),
            })
            .await;

        let panes = match client.recv().await.msg {
            Some(proto::server_message::Msg::Snapshot(snapshot)) => snapshot.panes,
            other => panic!("expected Snapshot, got {other:?}"),
        };
        let pane = panes
            .iter()
            .find(|pane| pane.pane_id == pane_id)
            .expect("reconstructed pane should be present");
        assert_eq!(pane.cwd, project_dir_string);

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::Input(proto::Input {
                    session_id: session_id.clone(),
                    pane_id: pane_id.clone(),
                    data: b"pwd\n".to_vec(),
                })),
            })
            .await;

        let output = collect_delta_text(&mut client, Duration::from_secs(2)).await;
        assert!(
            output.contains(&project_dir_string),
            "reconstructed shell should start in the last reported cwd.\noutput:\n{output}"
        );
    }
}

fn shell_quote(value: &str) -> String {
    value.replace('\'', "'\"'\"'")
}

async fn collect_delta_text(client: &mut TestClient, window: Duration) -> String {
    let messages = client.drain(window).await;
    let mut output = Vec::new();
    for message in messages {
        if let Some(proto::server_message::Msg::Delta(delta)) = message.msg {
            output.extend(delta.data);
        }
    }
    String::from_utf8_lossy(&output).to_string()
}
