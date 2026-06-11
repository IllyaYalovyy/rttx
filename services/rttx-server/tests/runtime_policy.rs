//! Integration tests for daemon workspace retention policy behavior.

mod common;

use common::{TestClient, list_workspaces, start_test_server, wait_for_state_containing};
use rttx_proto::v3;
use std::time::Duration;

#[tokio::test]
async fn ephemeral_workspace_terminates_on_last_explicit_detach() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
                name: "ephemeral-detach".into(),
                policy: v3::WorkspacePolicy::Ephemeral as i32,
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
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    assert!(matches!(
        client.recv().await.payload,
        Some(v3::server_envelope::Payload::WorkspaceSnapshot(_))
    ));

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::DetachWorkspace(v3::DetachWorkspace {
                runtime_id: runtime_id.clone(),
            })),
        })
        .await;
    match client.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceTerminated(terminated)) => {
            assert_eq!(terminated.runtime_id, runtime_id);
            assert_eq!(terminated.final_revision, 3);
            assert_eq!(terminated.reason, v3::WorkspaceTerminationReason::EphemeralDetach as i32);
        }
        other => panic!("expected WorkspaceTerminated, got {other:?}"),
    }

    let mut observer = TestClient::connect(&sock).await;
    observer.handshake().await;
    let workspaces = list_workspaces(&mut observer).await;
    assert!(workspaces.is_empty());
}

#[tokio::test]
async fn ephemeral_workspace_survives_transport_disconnect() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let runtime_id = {
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        client
            .send(&v3::ClientEnvelope {
                request_id: 0,
                command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
                    name: "ephemeral-disconnect".into(),
                    policy: v3::WorkspacePolicy::Ephemeral as i32,
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
                command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                    runtime_id: runtime_id.clone(),
                    attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
                })),
            })
            .await;
        assert!(matches!(
            client.recv().await.payload,
            Some(v3::server_envelope::Payload::WorkspaceSnapshot(_))
        ));

        runtime_id
    };

    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut reconnect = TestClient::connect(&sock).await;
    reconnect.handshake().await;
    let workspaces = list_workspaces(&mut reconnect).await;
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].id, runtime_id);
    assert_eq!(
        v3::WorkspacePolicy::try_from(workspaces[0].policy).unwrap(),
        v3::WorkspacePolicy::Ephemeral
    );
    assert_eq!(workspaces[0].read_only_client_count, 0);
    assert_eq!(workspaces[0].current_client_role, v3::WorkspaceClientRole::Unattached as i32);

    reconnect
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match reconnect.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceSnapshot(snapshot)) => {
            assert_eq!(snapshot.runtime_id, runtime_id);
            assert_eq!(snapshot.client_role, v3::WorkspaceClientRole::Writer as i32);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }
}

#[tokio::test]
async fn ephemeral_workspace_is_not_restored_after_restart() {
    let tmp = tempfile::TempDir::new().unwrap();

    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        client
            .send(&v3::ClientEnvelope {
                request_id: 0,
                command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
                    name: "e-policy-test".into(),
                    policy: v3::WorkspacePolicy::Ephemeral as i32,
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
                    runtime_id,
                    cwd: None,
                    dark_background: None,
                    cols: 0,
                    rows: 0,
                    no_persist: None,
                })),
            })
            .await;
        assert!(matches!(
            client.recv().await.payload,
            Some(v3::server_envelope::Payload::PaneCreated(_))
        ));

        // Create a persistent workspace as anchor to wait for serialization.
        let _ =
            common::create_workspace(&mut client, "e-policy-anchor", v3::WorkspacePolicy::Persistent)
                .await;
        wait_for_state_containing(tmp.path(), "e-policy-anchor", Duration::from_secs(10)).await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        let workspaces = list_workspaces(&mut client).await;
        assert_eq!(
            workspaces.len(),
            1,
            "only the persistent anchor should survive, not the ephemeral workspace"
        );
        assert_eq!(workspaces[0].name, "e-policy-anchor");
    }
}
