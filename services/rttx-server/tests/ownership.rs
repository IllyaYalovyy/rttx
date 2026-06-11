//! Integration tests for workspace ownership and single-writer attach semantics.

mod common;

use common::{TestClient, list_workspaces, start_test_server};
use rttx_proto::v3;

#[tokio::test]
async fn second_writer_attach_returns_attach_blocked() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;

    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
                name: "writer-conflict".into(),
                policy: v3::WorkspacePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match writer.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(created)) => created.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match writer.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceSnapshot(snapshot)) => {
            assert_eq!(snapshot.client_role, v3::WorkspaceClientRole::Writer as i32);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    let mut second = TestClient::connect(&sock).await;
    second.handshake().await;
    second
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match second.recv().await.payload {
        Some(v3::server_envelope::Payload::AttachBlocked(blocked)) => {
            assert_eq!(blocked.runtime_id, runtime_id);
            assert_eq!(blocked.current_client_role, v3::WorkspaceClientRole::Unattached as i32);
            assert_eq!(blocked.read_only_client_count, 0);
        }
        other => panic!("expected AttachBlocked, got {other:?}"),
    }

    let workspaces = list_workspaces(&mut second).await;
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].current_client_role, v3::WorkspaceClientRole::Unattached as i32);
    assert!(workspaces[0].has_write_owner);
    assert_eq!(workspaces[0].read_only_client_count, 0);
}

#[tokio::test]
async fn read_only_attach_cannot_mutate_workspace() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;

    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
                name: "reader-denied".into(),
                policy: v3::WorkspacePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match writer.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(created)) => created.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match writer.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceSnapshot(snapshot)) => {
            assert_eq!(snapshot.workspace_revision, 2);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    reader
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::WorkspaceAttachMode::ReadOnly as i32,
            })),
        })
        .await;
    match reader.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceSnapshot(snapshot)) => {
            assert_eq!(snapshot.workspace_revision, 3);
            assert_eq!(snapshot.client_role, v3::WorkspaceClientRole::Reader as i32);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    reader
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
    match reader.recv().await.payload {
        Some(v3::server_envelope::Payload::Error(error)) => {
            assert_eq!(error.kind, v3::ErrorKind::OwnershipConflict as i32);
            assert!(error.message.contains("owned by another client"));
        }
        other => panic!("expected Error, got {other:?}"),
    }

    let workspaces = list_workspaces(&mut reader).await;
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].workspace_revision, 3);
    assert_eq!(workspaces[0].current_client_role, v3::WorkspaceClientRole::Reader as i32);
    assert!(workspaces[0].has_write_owner);
    assert_eq!(workspaces[0].read_only_client_count, 1);
}

#[tokio::test]
async fn terminate_workspace_notifies_other_attached_clients_and_removes_state() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;

    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
                name: "terminate-workspace".into(),
                policy: v3::WorkspacePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match writer.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(created)) => created.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    assert!(matches!(
        writer.recv().await.payload,
        Some(v3::server_envelope::Payload::WorkspaceSnapshot(_))
    ));

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    reader
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::WorkspaceAttachMode::ReadOnly as i32,
            })),
        })
        .await;
    assert!(matches!(
        reader.recv().await.payload,
        Some(v3::server_envelope::Payload::WorkspaceSnapshot(_))
    ));

    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::TerminateWorkspace(v3::TerminateWorkspace {
                runtime_id: runtime_id.clone(),
            })),
        })
        .await;
    match writer.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceTerminated(terminated)) => {
            assert_eq!(terminated.runtime_id, runtime_id);
            assert_eq!(terminated.final_revision, 4);
            assert_eq!(terminated.reason, v3::WorkspaceTerminationReason::Explicit as i32);
        }
        other => panic!("expected WorkspaceTerminated, got {other:?}"),
    }

    match reader.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceTerminated(terminated)) => {
            assert_eq!(terminated.runtime_id, runtime_id);
            assert_eq!(terminated.final_revision, 4);
            assert_eq!(terminated.reason, v3::WorkspaceTerminationReason::Explicit as i32);
        }
        other => panic!("expected pushed WorkspaceTerminated, got {other:?}"),
    }

    let mut third = TestClient::connect(&sock).await;
    third.handshake().await;
    let workspaces = list_workspaces(&mut third).await;
    assert!(workspaces.is_empty());
}

#[tokio::test]
async fn read_only_client_cannot_rename_workspace() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;

    let runtime_id =
        common::create_workspace(&mut writer, "rename-denied", v3::WorkspacePolicy::Persistent).await;
    common::attach_rw(&mut writer, &runtime_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    common::attach_ro(&mut reader, &runtime_id).await;

    reader
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::RenameWorkspace(v3::RenameWorkspace {
                runtime_id: runtime_id.clone(),
                name: "hijacked".into(),
            })),
        })
        .await;
    match reader.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::Error(e)) => {
            assert_eq!(e.kind, v3::ErrorKind::OwnershipConflict as i32);
        }
        other => panic!("expected Error, got {other:?}"),
    }

    // Verify name unchanged.
    let workspaces = list_workspaces(&mut writer).await;
    assert_eq!(workspaces[0].name, "rename-denied");
}

#[tokio::test]
async fn read_only_client_cannot_set_pane_title() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;

    let runtime_id =
        common::create_workspace(&mut writer, "title-denied", v3::WorkspacePolicy::Persistent).await;
    common::attach_rw(&mut writer, &runtime_id).await;
    let pane_id = common::create_pane(&mut writer, &runtime_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    common::attach_ro(&mut reader, &runtime_id).await;

    reader
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::SetPaneTitle(v3::SetPaneTitle {
                runtime_id: runtime_id.clone(),
                pane_id,
                title: "hijacked".into(),
            })),
        })
        .await;

    // SetPaneTitle is fire-and-forget; the server silently drops it for a
    // read-only client. Use a Ping/Pong barrier to flush, then confirm the
    // reader sees neither an error nor a TitleChanged broadcast — proving
    // the title was never changed.
    reader.ping().await;
    let events = reader.drain(std::time::Duration::from_millis(200)).await;
    assert!(
        events.iter().all(|e| !matches!(
            e.payload,
            Some(
                v3::server_envelope::Payload::Error(_)
                    | v3::server_envelope::Payload::TitleChanged(_)
            )
        )),
        "read-only client must not be able to change the pane title"
    );
}
