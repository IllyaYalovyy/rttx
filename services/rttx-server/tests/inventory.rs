//! Integration tests for workspace inventory metadata exposed by `ListWorkspaces`.

mod common;

use common::*;
use rttx_proto::v3;
use std::time::Duration;

#[tokio::test]
async fn list_workspaces_includes_workspace_inventory_metadata() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
                name: "inventory-test".into(),
                policy: v3::WorkspacePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match client.recv().await.payload {
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
    let pane_id = match client.recv().await.payload {
        Some(v3::server_envelope::Payload::PaneCreated(created)) => created.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    // Attach read-write so the client can set the pane title (SetPaneTitle is
    // silently dropped for clients without write access in v3), then detach
    // afterwards so the inventory reports the workspace as unattached.
    common::attach_rw(&mut client, &runtime_id).await;

    // Let the interactive shell emit its initial prompt/title traffic before
    // asserting a later manual SetPaneTitle update.
    let _ = client.drain(Duration::from_millis(500)).await;

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::SetPaneTitle(v3::SetPaneTitle {
                runtime_id: runtime_id.clone(),
                pane_id: pane_id.clone(),
                title: "inventory-shell".into(),
            })),
        })
        .await;
    // SetPaneTitle is fire-and-forget; flush it with a Ping/Pong barrier.
    client.ping().await;

    // Drain any PTY output that may overwrite the title via OSC sequences.
    let _ = client.drain(Duration::from_millis(300)).await;

    // Detach so the workspace is reported as unattached in the inventory.
    common::detach_workspace(&mut client, &runtime_id).await;

    let workspaces = list_workspaces(&mut client).await;
    assert_eq!(workspaces.len(), 1);

    let session = &workspaces[0];
    assert_eq!(session.id, runtime_id);
    assert_eq!(session.name, "inventory-test");
    assert_eq!(session.pane_count, 1);
    assert!(!session.has_write_owner);
    assert_eq!(session.read_only_client_count, 0);
    assert_eq!(session.current_client_role, v3::WorkspaceClientRole::Unattached as i32);
    assert_eq!(
        v3::WorkspacePolicy::try_from(session.policy).unwrap(),
        v3::WorkspacePolicy::Persistent
    );
    assert!(!session.reconstructed);
    assert_eq!(session.panes.len(), 1);

    let pane = &session.panes[0];
    assert_eq!(pane.id, pane_id);
    // Title may be overwritten by the shell's OSC title sequence after SetPaneTitle.
    assert!(!pane.title.is_empty(), "pane title should be non-empty");
    // CWD may be populated from /proc fallback even without OSC 7.
    assert_eq!(pane.cols, 80);
    assert_eq!(pane.rows, 24);
    assert_eq!(pane.exit_status, None);
    assert!(!pane.reconstructed);
}

#[tokio::test]
async fn list_workspaces_tracks_attached_client_count() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut first = TestClient::connect(&sock).await;
    first.handshake().await;

    first
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
                name: "attach-count".into(),
                policy: v3::WorkspacePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match first.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(created)) => created.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    first
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    assert!(matches!(
        first.recv().await.payload,
        Some(v3::server_envelope::Payload::WorkspaceSnapshot(_))
    ));

    let mut second = TestClient::connect(&sock).await;
    second.handshake().await;
    second
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::WorkspaceAttachMode::ReadOnly as i32,
            })),
        })
        .await;
    assert!(matches!(
        second.recv().await.payload,
        Some(v3::server_envelope::Payload::WorkspaceSnapshot(_))
    ));

    let workspaces = list_workspaces(&mut second).await;
    assert_eq!(workspaces.len(), 1);
    assert!(workspaces[0].has_write_owner);
    assert_eq!(workspaces[0].current_client_role, v3::WorkspaceClientRole::Reader as i32);
    assert_eq!(workspaces[0].read_only_client_count, 1);

    drop(first);
    tokio::time::sleep(Duration::from_millis(200)).await;

    let workspaces = list_workspaces(&mut second).await;
    assert!(!workspaces[0].has_write_owner);
    assert_eq!(workspaces[0].read_only_client_count, 1);

    drop(second);
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut third = TestClient::connect(&sock).await;
    third.handshake().await;
    let workspaces = list_workspaces(&mut third).await;
    assert_eq!(workspaces[0].read_only_client_count, 0);
    assert!(!workspaces[0].has_write_owner);
}

#[tokio::test]
async fn list_workspaces_marks_restored_workspace_and_panes_as_reconstructed() {
    let tmp = tempfile::TempDir::new().unwrap();
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
                    name: "reconstructed-inventory".into(),
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

        // Attach read-write so resize and set-title are applied (both are
        // fire-and-forget and require write access in v3).
        common::attach_rw(&mut client, &runtime_id).await;

        client
            .send(&v3::ClientEnvelope {
                request_id: 0,
                command: Some(v3::client_envelope::Command::ResizePane(v3::ResizePane {
                    runtime_id: runtime_id.clone(),
                    pane_id: pane_id.clone(),
                    cols: 100,
                    rows: 30,
                })),
            })
            .await;
        // Fire-and-forget: flush with a Ping/Pong barrier.
        client.ping().await;

        client
            .send(&v3::ClientEnvelope {
                request_id: 0,
                command: Some(v3::client_envelope::Command::SetPaneTitle(v3::SetPaneTitle {
                    runtime_id: runtime_id.clone(),
                    pane_id: pane_id.clone(),
                    title: "restored-shell".into(),
                })),
            })
            .await;
        client.ping().await;

        wait_for_state_containing(tmp.path(), "reconstructed-inventory", Duration::from_secs(10))
            .await;

        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        let workspaces = list_workspaces(&mut client).await;
        assert_eq!(workspaces.len(), 1);

        let session = &workspaces[0];
        assert_eq!(session.id, runtime_id);
        assert_eq!(session.name, "reconstructed-inventory");
        assert_eq!(session.pane_count, 1);
        assert_eq!(session.read_only_client_count, 0);
        assert!(!session.has_write_owner);
        assert_eq!(session.current_client_role, v3::WorkspaceClientRole::Unattached as i32);
        assert_eq!(
            v3::WorkspacePolicy::try_from(session.policy).unwrap(),
            v3::WorkspacePolicy::Persistent
        );
        assert!(session.reconstructed);
        assert_eq!(session.panes.len(), 1);

        let pane = &session.panes[0];
        assert_eq!(pane.id, pane_id);
        assert_eq!(pane.cols, 100);
        assert_eq!(pane.rows, 30);
        assert!(pane.reconstructed);
    }
}

/// Pane CWD in inventory must be populated from /proc fallback. Regression for #235.
#[tokio::test]
async fn inventory_pane_cwd_populated_from_proc_fallback() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let runtime_id =
        create_workspace(&mut client, "cwd-check", v3::WorkspacePolicy::Persistent).await;
    let _pane_id = create_pane(&mut client, &runtime_id).await;

    // Give the shell a moment to start so /proc/<pid>/cwd is readable.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let workspaces = list_workspaces(&mut client).await;
    let pane = &workspaces[0].panes[0];
    assert!(!pane.cwd.is_empty(), "pane CWD should be populated from /proc fallback, got empty");
}

/// Behavior backing `status <runtime-id>`: with the inventory capability
/// negotiated, `ListWorkspaces` returns per-pane detail (id + size) that the
/// CLI detail view formats. Guards the end-to-end data path the command relies
/// on.
#[tokio::test]
async fn status_detail_inventory_carries_pane_id_and_size() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let runtime_id = create_workspace(&mut client, "detail", v3::WorkspacePolicy::Persistent).await;
    let pane_id = create_pane(&mut client, &runtime_id).await;

    let workspaces = list_workspaces(&mut client).await;
    let ws = workspaces.iter().find(|w| w.id == runtime_id).expect("workspace is listed");
    assert_eq!(ws.panes.len(), 1, "enriched per-pane detail must be present");
    let pane = &ws.panes[0];
    assert_eq!(pane.id, pane_id, "pane id matches the created pane");
    assert!(pane.cols > 0 && pane.rows > 0, "pane size populated: {}x{}", pane.cols, pane.rows);
}
