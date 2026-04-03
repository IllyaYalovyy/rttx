//! Integration tests for daemon runtime retention policy behavior.

mod common;

use common::{TestClient, list_sessions, start_test_server, wait_for_state_containing};
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
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: "ephemeral-detach".into(),
                policy: proto::RuntimePolicy::Ephemeral as i32,
            })),
        })
        .await;
    let session_id = match client.recv().await.msg {
        Some(proto::server_message::Msg::SessionCreated(created)) => created.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    assert!(matches!(client.recv().await.msg, Some(proto::server_message::Msg::Snapshot(_))));

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::DetachSession(proto::DetachSession {
                session_id: session_id.clone(),
            })),
        })
        .await;
    match client.recv().await.msg {
        Some(proto::server_message::Msg::SessionTerminated(terminated)) => {
            assert_eq!(terminated.session_id, session_id);
            assert_eq!(terminated.final_revision, 3);
            assert_eq!(
                terminated.reason,
                proto::RuntimeTerminationReason::EphemeralLastDetach as i32
            );
        }
        other => panic!("expected SessionTerminated, got {other:?}"),
    }

    let mut observer = TestClient::connect(&sock).await;
    observer.handshake().await;
    let sessions = list_sessions(&mut observer).await;
    assert!(sessions.is_empty());
}

#[tokio::test]
async fn ephemeral_runtime_survives_transport_disconnect() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let session_id = {
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                    name: "ephemeral-disconnect".into(),
                    policy: proto::RuntimePolicy::Ephemeral as i32,
                })),
            })
            .await;
        let session_id = match client.recv().await.msg {
            Some(proto::server_message::Msg::SessionCreated(created)) => created.session_id,
            other => panic!("expected SessionCreated, got {other:?}"),
        };

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                    session_id: session_id.clone(),
                    attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
                })),
            })
            .await;
        assert!(matches!(client.recv().await.msg, Some(proto::server_message::Msg::Snapshot(_))));

        session_id
    };

    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut reconnect = TestClient::connect(&sock).await;
    reconnect.handshake().await;
    let sessions = list_sessions(&mut reconnect).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, session_id);
    assert_eq!(
        proto::RuntimePolicy::try_from(sessions[0].policy).unwrap(),
        proto::RuntimePolicy::Ephemeral
    );
    assert_eq!(sessions[0].attached_client_count, 0);
    assert_eq!(sessions[0].current_client_role, proto::RuntimeClientRole::Unattached as i32);

    reconnect
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match reconnect.recv().await.msg {
        Some(proto::server_message::Msg::Snapshot(snapshot)) => {
            assert_eq!(snapshot.session_id, session_id);
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
                msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                    name: "serialized_at".into(),
                    policy: proto::RuntimePolicy::Ephemeral as i32,
                })),
            })
            .await;
        let session_id = match client.recv().await.msg {
            Some(proto::server_message::Msg::SessionCreated(created)) => created.session_id,
            other => panic!("expected SessionCreated, got {other:?}"),
        };

        client
            .send(&proto::ClientMessage {
                msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane { session_id })),
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

        let sessions = list_sessions(&mut client).await;
        assert!(sessions.is_empty());
    }
}
