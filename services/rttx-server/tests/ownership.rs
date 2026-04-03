//! Integration tests for runtime ownership and single-writer attach semantics.

mod common;

use common::{TestClient, list_sessions, start_test_server};
use rttx_proto::proto;

#[tokio::test]
async fn second_writer_attach_returns_attach_blocked() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;

    writer
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: "writer-conflict".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let session_id = match writer.recv().await.msg {
        Some(proto::server_message::Msg::SessionCreated(created)) => created.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    writer
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match writer.recv().await.msg {
        Some(proto::server_message::Msg::Snapshot(snapshot)) => {
            assert_eq!(snapshot.current_client_role, proto::RuntimeClientRole::Writer as i32);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    let mut second = TestClient::connect(&sock).await;
    second.handshake().await;
    second
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match second.recv().await.msg {
        Some(proto::server_message::Msg::AttachBlocked(blocked)) => {
            assert_eq!(blocked.session_id, session_id);
            assert_eq!(blocked.current_client_role, proto::RuntimeClientRole::Unattached as i32);
            assert_eq!(blocked.attached_client_count, 1);
            assert_eq!(blocked.read_only_client_count, 0);
        }
        other => panic!("expected AttachBlocked, got {other:?}"),
    }

    let sessions = list_sessions(&mut second).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].current_client_role, proto::RuntimeClientRole::Unattached as i32);
    assert!(sessions[0].has_write_owner);
    assert_eq!(sessions[0].attached_client_count, 1);
    assert_eq!(sessions[0].read_only_client_count, 0);
}

#[tokio::test]
async fn read_only_attach_cannot_mutate_runtime() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;

    writer
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: "reader-denied".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let session_id = match writer.recv().await.msg {
        Some(proto::server_message::Msg::SessionCreated(created)) => created.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    writer
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match writer.recv().await.msg {
        Some(proto::server_message::Msg::Snapshot(snapshot)) => {
            assert_eq!(snapshot.revision, 2);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    reader
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadOnly as i32,
            })),
        })
        .await;
    match reader.recv().await.msg {
        Some(proto::server_message::Msg::Snapshot(snapshot)) => {
            assert_eq!(snapshot.revision, 3);
            assert_eq!(snapshot.current_client_role, proto::RuntimeClientRole::Reader as i32);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    reader
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                session_id: session_id.clone(),
            })),
        })
        .await;
    match reader.recv().await.msg {
        Some(proto::server_message::Msg::Error(error)) => {
            assert_eq!(error.code, 9);
            assert!(error.message.contains("owned by another client"));
        }
        other => panic!("expected Error, got {other:?}"),
    }

    let sessions = list_sessions(&mut reader).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].revision, 3);
    assert_eq!(sessions[0].current_client_role, proto::RuntimeClientRole::Reader as i32);
    assert!(sessions[0].has_write_owner);
    assert_eq!(sessions[0].read_only_client_count, 1);
}

#[tokio::test]
async fn terminate_runtime_notifies_other_attached_clients_and_removes_state() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;

    writer
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: "terminate-runtime".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let session_id = match writer.recv().await.msg {
        Some(proto::server_message::Msg::SessionCreated(created)) => created.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    writer
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    assert!(matches!(writer.recv().await.msg, Some(proto::server_message::Msg::Snapshot(_))));

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    reader
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadOnly as i32,
            })),
        })
        .await;
    assert!(matches!(reader.recv().await.msg, Some(proto::server_message::Msg::Snapshot(_))));

    writer
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::TerminateSession(proto::TerminateSession {
                session_id: session_id.clone(),
            })),
        })
        .await;
    match writer.recv().await.msg {
        Some(proto::server_message::Msg::SessionTerminated(terminated)) => {
            assert_eq!(terminated.session_id, session_id);
            assert_eq!(terminated.final_revision, 4);
            assert_eq!(terminated.reason, proto::RuntimeTerminationReason::Explicit as i32);
        }
        other => panic!("expected SessionTerminated, got {other:?}"),
    }

    match reader.recv().await.msg {
        Some(proto::server_message::Msg::SessionTerminated(terminated)) => {
            assert_eq!(terminated.session_id, session_id);
            assert_eq!(terminated.final_revision, 4);
            assert_eq!(terminated.reason, proto::RuntimeTerminationReason::Explicit as i32);
        }
        other => panic!("expected pushed SessionTerminated, got {other:?}"),
    }

    let mut third = TestClient::connect(&sock).await;
    third.handshake().await;
    let sessions = list_sessions(&mut third).await;
    assert!(sessions.is_empty());
}
