//! Integration tests for session lifecycle.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::v3;
use std::time::Duration;

#[tokio::test]
async fn create_workspace_and_list() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    // Create a session.
    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: "test-session".into(),
            policy: v3::WorkspacePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let resp = client.recv().await;
    let runtime_id = match resp.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(sc)) => sc.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };
    assert_eq!(runtime_id.len(), 16);

    // List workspaces.
    let list = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::ListWorkspaces(v3::ListWorkspaces {})),
    };
    client.send(&list).await;
    let resp = client.recv().await;
    match resp.payload {
        Some(v3::server_envelope::Payload::WorkspaceList(sl)) => {
            assert_eq!(sl.workspaces.len(), 1);
            assert_eq!(sl.workspaces[0].name, "test-session");
        }
        other => panic!("expected WorkspaceList, got {other:?}"),
    }
}

#[tokio::test]
async fn attach_and_detach_workspace() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    // Create session.
    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: "attach-test".into(),
            policy: v3::WorkspacePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let resp = client.recv().await;
    let runtime_id = match resp.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(sc)) => sc.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    // Attach.
    let attach = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
            runtime_id: runtime_id.clone(),
            attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    let resp = client.recv().await;
    match resp.payload {
        Some(v3::server_envelope::Payload::WorkspaceSnapshot(snap)) => {
            assert_eq!(snap.runtime_id, runtime_id);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    // Detach.
    let detach = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::DetachWorkspace(v3::DetachWorkspace {
            runtime_id: runtime_id.clone(),
        })),
    };
    client.send(&detach).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for WorkspaceDetached");
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceDetached(_)) => break,
            Some(
                v3::server_envelope::Payload::OutputDelta(_)
                | v3::server_envelope::Payload::PaneExited(_),
            ) => {}
            other => panic!("expected WorkspaceDetached, got {other:?}"),
        }
    }

    // Verify session still exists after detach.
    let list = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::ListWorkspaces(v3::ListWorkspaces {})),
    };
    client.send(&list).await;
    let resp = client.recv().await;
    match resp.payload {
        Some(v3::server_envelope::Payload::WorkspaceList(sl)) => {
            assert_eq!(sl.workspaces.len(), 1);
        }
        other => panic!("expected WorkspaceList, got {other:?}"),
    }
}

#[tokio::test]
async fn create_and_close_pane() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    // Create session.
    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: "pane-test".into(),
            policy: v3::WorkspacePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let resp = client.recv().await;
    let runtime_id = match resp.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(sc)) => sc.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    // Create pane.
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
    let pane_id = match resp.payload {
        Some(v3::server_envelope::Payload::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    // Close pane.
    let close_pane = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::ClosePane(v3::ClosePane {
            runtime_id: runtime_id.clone(),
            pane_id,
        })),
    };
    client.send(&close_pane).await;
    let resp = client.recv().await;
    assert!(matches!(resp.payload, Some(v3::server_envelope::Payload::PaneClosed(_))));
}

#[tokio::test]
async fn rename_workspace_updates_name_and_inventory() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let runtime_id =
        common::create_workspace(&mut client, "original", v3::WorkspacePolicy::Persistent).await;
    common::attach_rw(&mut client, &runtime_id).await;

    // Rename the session.
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::RenameWorkspace(v3::RenameWorkspace {
                runtime_id: runtime_id.clone(),
                name: "renamed".into(),
            })),
        })
        .await;

    // Expect WorkspaceRenamed response.
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceRenamed(renamed)) => {
                assert_eq!(renamed.runtime_id, runtime_id);
                assert_eq!(renamed.name, "renamed");
                break;
            }
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected WorkspaceRenamed, got {other:?}"),
        }
    }

    // Verify inventory reflects the new name.
    let workspaces = common::list_workspaces(&mut client).await;
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].name, "renamed");
}

#[tokio::test]
async fn rename_workspace_persists_across_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let runtime_id =
        common::create_workspace(&mut client, "before", v3::WorkspacePolicy::Persistent).await;
    common::attach_rw(&mut client, &runtime_id).await;

    // Rename.
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::RenameWorkspace(v3::RenameWorkspace {
                runtime_id: runtime_id.clone(),
                name: "after".into(),
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceRenamed(_)) => break,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected WorkspaceRenamed, got {other:?}"),
        }
    }

    // Wait for state to be persisted with the new name.
    common::wait_for_state_containing(tmp.path(), "after", Duration::from_secs(5)).await;

    // Shutdown and restart.
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::Shutdown(v3::Shutdown {})),
        })
        .await;
    let _ = handle.await;

    let (socket_path2, _handle2) = start_test_server(tmp.path()).await;
    let mut client2 = TestClient::connect(&socket_path2).await;
    client2.handshake().await;

    let workspaces = common::list_workspaces(&mut client2).await;
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].name, "after");
}
