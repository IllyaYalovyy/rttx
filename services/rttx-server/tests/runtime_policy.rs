//! Integration tests for daemon runtime retention policy behavior.

mod common;

use common::{TestClient, list_runtimes, start_test_server, wait_for_state_containing};
use rttx_proto::v3;
use std::time::Duration;

#[tokio::test]
async fn ephemeral_runtime_terminates_on_last_explicit_detach() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
                name: "ephemeral-detach".into(),
                policy: v3::RuntimePolicy::Ephemeral as i32,
            })),
        })
        .await;
    let runtime_id = match client.recv().await.payload {
        Some(v3::server_envelope::Payload::RuntimeCreated(created)) => created.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    assert!(matches!(
        client.recv().await.payload,
        Some(v3::server_envelope::Payload::RuntimeSnapshot(_))
    ));

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::DetachRuntime(v3::DetachRuntime {
                runtime_id: runtime_id.clone(),
            })),
        })
        .await;
    match client.recv().await.payload {
        Some(v3::server_envelope::Payload::RuntimeTerminated(terminated)) => {
            assert_eq!(terminated.runtime_id, runtime_id);
            assert_eq!(terminated.final_revision, 3);
            assert_eq!(terminated.reason, v3::RuntimeTerminationReason::EphemeralDetach as i32);
        }
        other => panic!("expected RuntimeTerminated, got {other:?}"),
    }

    let mut observer = TestClient::connect(&sock).await;
    observer.handshake().await;
    let runtimes = list_runtimes(&mut observer).await;
    assert!(runtimes.is_empty());
}

#[tokio::test]
async fn ephemeral_runtime_survives_transport_disconnect() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let runtime_id = {
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        client
            .send(&v3::ClientEnvelope {
                request_id: 0,
                command: Some(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
                    name: "ephemeral-disconnect".into(),
                    policy: v3::RuntimePolicy::Ephemeral as i32,
                })),
            })
            .await;
        let runtime_id = match client.recv().await.payload {
            Some(v3::server_envelope::Payload::RuntimeCreated(created)) => created.runtime_id,
            other => panic!("expected RuntimeCreated, got {other:?}"),
        };

        client
            .send(&v3::ClientEnvelope {
                request_id: 0,
                command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
                    runtime_id: runtime_id.clone(),
                    attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
                })),
            })
            .await;
        assert!(matches!(
            client.recv().await.payload,
            Some(v3::server_envelope::Payload::RuntimeSnapshot(_))
        ));

        runtime_id
    };

    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut reconnect = TestClient::connect(&sock).await;
    reconnect.handshake().await;
    let runtimes = list_runtimes(&mut reconnect).await;
    assert_eq!(runtimes.len(), 1);
    assert_eq!(runtimes[0].id, runtime_id);
    assert_eq!(
        v3::RuntimePolicy::try_from(runtimes[0].policy).unwrap(),
        v3::RuntimePolicy::Ephemeral
    );
    assert_eq!(runtimes[0].read_only_client_count, 0);
    assert_eq!(runtimes[0].current_client_role, v3::RuntimeClientRole::Unattached as i32);

    reconnect
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match reconnect.recv().await.payload {
        Some(v3::server_envelope::Payload::RuntimeSnapshot(snapshot)) => {
            assert_eq!(snapshot.runtime_id, runtime_id);
            assert_eq!(snapshot.client_role, v3::RuntimeClientRole::Writer as i32);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }
}

#[tokio::test]
async fn ephemeral_runtime_is_not_restored_after_restart() {
    let tmp = tempfile::TempDir::new().unwrap();

    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        client
            .send(&v3::ClientEnvelope {
                request_id: 0,
                command: Some(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
                    name: "e-policy-test".into(),
                    policy: v3::RuntimePolicy::Ephemeral as i32,
                })),
            })
            .await;
        let runtime_id = match client.recv().await.payload {
            Some(v3::server_envelope::Payload::RuntimeCreated(created)) => created.runtime_id,
            other => panic!("expected RuntimeCreated, got {other:?}"),
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

        // Create a persistent runtime as anchor to wait for serialization.
        let _ =
            common::create_runtime(&mut client, "e-policy-anchor", v3::RuntimePolicy::Persistent)
                .await;
        wait_for_state_containing(tmp.path(), "e-policy-anchor", Duration::from_secs(10)).await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        let runtimes = list_runtimes(&mut client).await;
        assert_eq!(
            runtimes.len(),
            1,
            "only the persistent anchor should survive, not the ephemeral runtime"
        );
        assert_eq!(runtimes[0].name, "e-policy-anchor");
    }
}
