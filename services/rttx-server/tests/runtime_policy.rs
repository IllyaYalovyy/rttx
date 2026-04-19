//! Integration tests for daemon runtime retention policy behavior.

mod common;

use common::{TestClient, list_runtimes, start_test_server, wait_for_state_containing};
use rttx_proto::proto;
use std::time::Duration;

#[tokio::test]
async fn ephemeral_runtime_terminates_on_last_explicit_detach() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
                name: "ephemeral-detach".into(),
                policy: proto::RuntimePolicy::Ephemeral as i32,
            })),
        })
        .await;
    let runtime_id = match client.recv().await.msg {
        Some(proto::server_message::Msg::RuntimeCreated(created)) => created.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
                runtime_id: runtime_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    assert!(matches!(client.recv().await.msg, Some(proto::server_message::Msg::Snapshot(_))));

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::DetachRuntime(proto::DetachRuntime {
                runtime_id: runtime_id.clone(),
            })),
        })
        .await;
    match client.recv().await.msg {
        Some(proto::server_message::Msg::RuntimeTerminated(terminated)) => {
            assert_eq!(terminated.runtime_id, runtime_id);
            assert_eq!(terminated.final_revision, 3);
            assert_eq!(
                terminated.reason,
                proto::RuntimeTerminationReason::EphemeralLastDetach as i32
            );
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
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
                    name: "ephemeral-disconnect".into(),
                    policy: proto::RuntimePolicy::Ephemeral as i32,
                })),
            })
            .await;
        let runtime_id = match client.recv().await.msg {
            Some(proto::server_message::Msg::RuntimeCreated(created)) => created.runtime_id,
            other => panic!("expected RuntimeCreated, got {other:?}"),
        };

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
                    runtime_id: runtime_id.clone(),
                    attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
                })),
            })
            .await;
        assert!(matches!(client.recv().await.msg, Some(proto::server_message::Msg::Snapshot(_))));

        runtime_id
    };

    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut reconnect = TestClient::connect(&sock).await;
    reconnect.handshake().await;
    let runtimes = list_runtimes(&mut reconnect).await;
    assert_eq!(runtimes.len(), 1);
    assert_eq!(runtimes[0].id, runtime_id);
    assert_eq!(
        proto::RuntimePolicy::try_from(runtimes[0].policy).unwrap(),
        proto::RuntimePolicy::Ephemeral
    );
    assert_eq!(runtimes[0].attached_client_count, 0);
    assert_eq!(runtimes[0].current_client_role, proto::RuntimeClientRole::Unattached as i32);

    reconnect
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
                runtime_id: runtime_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match reconnect.recv().await.msg {
        Some(proto::server_message::Msg::Snapshot(snapshot)) => {
            assert_eq!(snapshot.runtime_id, runtime_id);
            assert_eq!(snapshot.current_client_role, proto::RuntimeClientRole::Writer as i32);
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
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
                    name: "serialized_at".into(),
                    policy: proto::RuntimePolicy::Ephemeral as i32,
                })),
            })
            .await;
        let runtime_id = match client.recv().await.msg {
            Some(proto::server_message::Msg::RuntimeCreated(created)) => created.runtime_id,
            other => panic!("expected RuntimeCreated, got {other:?}"),
        };

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                    runtime_id,
                    cwd: None,
                    dark_background: None,
                    cols: 0,
                    rows: 0,
                })),
            })
            .await;
        assert!(matches!(
            client.recv().await.msg,
            Some(proto::server_message::Msg::PaneCreated(_))
        ));

        wait_for_state_containing(
            &tmp.path().join("cache"),
            "serialized_at",
            Duration::from_secs(10),
        )
        .await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        let runtimes = list_runtimes(&mut client).await;
        assert!(runtimes.is_empty());
    }
}
