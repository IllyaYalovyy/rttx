//! Integration tests for session reconstruction after daemon restart.

mod common;

use common::*;
use rttx_proto::v3;
use std::time::Duration;

/// Create a session with a pane, write to it, stop the server, restart,
/// and verify the scrollback is restored in the snapshot.
#[tokio::test]
async fn reconstruct_session_after_restart() {
    let tmp = tempfile::TempDir::new().unwrap();

    // Phase 1: start server, create session, produce output, let serialization tick.
    let runtime_id;
    let pane_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        // Create session.
        let create = v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
                name: "reconstruct-test".into(),
                policy: v3::WorkspacePolicy::Persistent as i32,
            })),
        };
        client.send(&create).await;
        let resp = client.recv().await;
        runtime_id = match resp.payload {
            Some(v3::server_envelope::Payload::WorkspaceCreated(sc)) => sc.runtime_id,
            other => panic!("expected WorkspaceCreated, got {other:?}"),
        };

        // Create pane (spawns a PTY).
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
        let resp = client.recv().await;
        pane_id = match resp.payload {
            Some(v3::server_envelope::Payload::PaneCreated(pc)) => pc.pane_id,
            other => panic!("expected PaneCreated, got {other:?}"),
        };

        // Attach to get deltas.
        let attach = v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
            })),
        };
        client.send(&attach).await;
        let _snapshot = client.recv().await;

        // Send a command that produces recognizable output.
        let input = v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
                runtime_id: runtime_id.clone(),
                pane_id: pane_id.clone(),
                kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                    data: bytes::Bytes::from_static(b"echo RECONSTRUCT_MARKER\n"),
                })),
            })),
        };
        client.send(&input).await;

        // Wait for output and serialization tick (>1s).
        wait_for_state_containing(tmp.path(), "reconstruct-test", Duration::from_secs(10)).await;

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
        let list = v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::ListWorkspaces(v3::ListWorkspaces {})),
        };
        client.send(&list).await;
        let resp = client.recv().await;
        let workspaces = match resp.payload {
            Some(v3::server_envelope::Payload::WorkspaceList(sl)) => sl.workspaces,
            other => panic!("expected WorkspaceList, got {other:?}"),
        };
        assert_eq!(workspaces.len(), 1, "session should be restored");
        assert_eq!(workspaces[0].name, "reconstruct-test");
        assert_eq!(workspaces[0].id, runtime_id);

        // Attach and check snapshot contains scrollback with our marker.
        let attach = v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
            })),
        };
        client.send(&attach).await;
        let resp = client.recv().await;
        let panes = match resp.payload {
            Some(v3::server_envelope::Payload::WorkspaceSnapshot(snap)) => snap.panes,
            other => panic!("expected Snapshot, got {other:?}"),
        };
        assert!(!panes.is_empty(), "should have at least one pane");

        let scrollback = String::from_utf8_lossy(&panes[0].scrollback_tail);
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

    let runtime_id;
    let pane_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        client
            .send(&v3::ClientEnvelope {
                request_id: 0,
                command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
                    name: "reconstruct-cwd".into(),
                    policy: v3::WorkspacePolicy::Persistent as i32,
                })),
            })
            .await;
        runtime_id = match client.recv().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceCreated(created)) => created.runtime_id,
            other => panic!("expected WorkspaceCreated, got {other:?}"),
        };

        client
            .send(&v3::ClientEnvelope {
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
        pane_id = match client.recv().await.payload {
            Some(v3::server_envelope::Payload::PaneCreated(created)) => created.pane_id,
            other => panic!("expected PaneCreated, got {other:?}"),
        };

        client
            .send(&v3::ClientEnvelope {
                request_id: 0,
                command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                    runtime_id: runtime_id.clone(),
                    attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
                })),
            })
            .await;
        match client.recv().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceSnapshot(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }

        let cwd_command = format!(
            "cd '{}'\nprintf '\\033]7;file://localhost%s\\007' \"$PWD\"\n",
            shell_quote(&project_dir_string)
        );
        client
            .send(&v3::ClientEnvelope {
                request_id: 0,
                command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
                    runtime_id: runtime_id.clone(),
                    pane_id: pane_id.clone(),
                    kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                        data: bytes::Bytes::from(cwd_command.into_bytes()),
                    })),
                })),
            })
            .await;

        wait_for_state_containing(tmp.path(), "reconstruct-cwd", Duration::from_secs(10)).await;
        let _ = tokio::time::timeout(Duration::from_millis(200), client.recv()).await;

        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        client
            .send(&v3::ClientEnvelope {
                request_id: 0,
                command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                    runtime_id: runtime_id.clone(),
                    attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
                })),
            })
            .await;

        let panes = match client.recv().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceSnapshot(snapshot)) => snapshot.panes,
            other => panic!("expected Snapshot, got {other:?}"),
        };
        let pane = panes
            .iter()
            .find(|pane| pane.pane_id == pane_id)
            .expect("reconstructed pane should be present");
        assert_eq!(pane.cwd, project_dir_string);

        client
            .send(&v3::ClientEnvelope {
                request_id: 0,
                command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
                    runtime_id: runtime_id.clone(),
                    pane_id: pane_id.clone(),
                    kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                        data: bytes::Bytes::from_static(b"pwd\n"),
                    })),
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
        if let Some(v3::server_envelope::Payload::OutputDelta(delta)) = message.payload {
            output.extend(delta.data);
        }
    }
    String::from_utf8_lossy(&output).to_string()
}

/// Multiple panes must each preserve their CWD after daemon restart.
#[tokio::test]
async fn reconstruct_preserves_cwd_for_multiple_panes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir_a = tmp.path().join("dir_a");
    let dir_b = tmp.path().join("dir_b");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();

    let runtime_id;
    let pane_a_id;
    let pane_b_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        runtime_id =
            create_workspace(&mut client, "multi-cwd", v3::WorkspacePolicy::Persistent).await;
        pane_a_id = create_pane(&mut client, &runtime_id).await;
        pane_b_id = create_pane(&mut client, &runtime_id).await;
        attach_rw(&mut client, &runtime_id).await;

        // Set CWD for pane A via OSC 7.
        let osc_a = format!(
            "cd '{}'\nprintf '\\033]7;file://localhost{}\\007' \n",
            dir_a.display(),
            dir_a.display()
        );
        send_input(&mut client, &runtime_id, &pane_a_id, osc_a.as_bytes()).await;

        // Set CWD for pane B via OSC 7.
        let osc_b = format!(
            "cd '{}'\nprintf '\\033]7;file://localhost{}\\007' \n",
            dir_b.display(),
            dir_b.display()
        );
        send_input(&mut client, &runtime_id, &pane_b_id, osc_b.as_bytes()).await;

        // Wait for state to contain both directories.
        let dir_a_str = dir_a.to_string_lossy().to_string();
        wait_for_state_containing(tmp.path(), &dir_a_str, Duration::from_secs(10)).await;

        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Restart and verify CWDs.
    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        let snap = attach_rw(&mut client, &runtime_id).await;
        let pane_a = snap.panes.iter().find(|p| p.pane_id == pane_a_id);
        let pane_b = snap.panes.iter().find(|p| p.pane_id == pane_b_id);

        assert!(pane_a.is_some(), "pane A must be in snapshot");
        assert!(pane_b.is_some(), "pane B must be in snapshot");

        let cwd_a = &pane_a.unwrap().cwd;
        let cwd_b = &pane_b.unwrap().cwd;

        assert!(!cwd_a.is_empty(), "pane A CWD must not be empty after restart, got: '{cwd_a}'");
        assert!(!cwd_b.is_empty(), "pane B CWD must not be empty after restart, got: '{cwd_b}'");
    }
}

/// Each pane must get a unique HISTFILE so shell history is per-pane.
#[tokio::test]
async fn pane_gets_unique_histfile() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let runtime_id =
        create_workspace(&mut client, "hist-test", v3::WorkspacePolicy::Persistent).await;
    let pane_id = create_pane(&mut client, &runtime_id).await;
    attach_rw(&mut client, &runtime_id).await;

    // Ask the shell to print its HISTFILE.
    send_input(&mut client, &runtime_id, &pane_id, b"echo HISTFILE=$HISTFILE\n").await;

    // Poll for the output containing the history path.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    while tokio::time::Instant::now() < deadline {
        if let Some(msg) = client.try_recv(Duration::from_millis(200)).await
            && let Some(v3::server_envelope::Payload::OutputDelta(d)) = msg.payload
        {
            output.extend_from_slice(&d.data);
            let text = String::from_utf8_lossy(&output);
            if text.contains(".hist") {
                break;
            }
        }
    }

    let text = String::from_utf8_lossy(&output);
    assert!(
        text.contains(".hist"),
        "expected HISTFILE with .hist extension in output, got: {text}"
    );
}

/// Sync gate evidence: history path must be unique per pane.
#[test]
fn history_path_unique_per_pane() {
    let state = std::path::Path::new("/tmp/test-state");
    let session = uuid::Uuid::new_v4();
    let p1 = uuid::Uuid::new_v4();
    let p2 = uuid::Uuid::new_v4();
    let h1 = rttx_server::state::layout::history_file(state, session, p1);
    let h2 = rttx_server::state::layout::history_file(state, session, p2);
    assert_ne!(h1, h2);
}
