//! Integration tests for runtime ownership and single-writer attach semantics.

mod common;

use common::{TestClient, list_runtimes, start_test_server};
use rttx_proto::proto;

#[tokio::test]
async fn second_writer_attach_returns_attach_blocked() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;

    writer
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
                name: "writer-conflict".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match writer.recv().await.msg {
        Some(proto::server_message::Msg::RuntimeCreated(created)) => created.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    writer
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
                runtime_id: runtime_id.clone(),
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
            msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
                runtime_id: runtime_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match second.recv().await.msg {
        Some(proto::server_message::Msg::AttachBlocked(blocked)) => {
            assert_eq!(blocked.runtime_id, runtime_id);
            assert_eq!(blocked.current_client_role, proto::RuntimeClientRole::Unattached as i32);
            assert_eq!(blocked.attached_client_count, 1);
            assert_eq!(blocked.read_only_client_count, 0);
        }
        other => panic!("expected AttachBlocked, got {other:?}"),
    }

    let runtimes = list_runtimes(&mut second).await;
    assert_eq!(runtimes.len(), 1);
    assert_eq!(runtimes[0].current_client_role, proto::RuntimeClientRole::Unattached as i32);
    assert!(runtimes[0].has_write_owner);
    assert_eq!(runtimes[0].attached_client_count, 1);
    assert_eq!(runtimes[0].read_only_client_count, 0);
}

#[tokio::test]
async fn read_only_attach_cannot_mutate_runtime() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;

    writer
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
                name: "reader-denied".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match writer.recv().await.msg {
        Some(proto::server_message::Msg::RuntimeCreated(created)) => created.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    writer
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
                runtime_id: runtime_id.clone(),
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
            msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
                runtime_id: runtime_id.clone(),
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
                runtime_id: runtime_id.clone(),
                cwd: None,
                dark_background: None,
                cols: 0,
                rows: 0,
                no_persist: None,
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

    let runtimes = list_runtimes(&mut reader).await;
    assert_eq!(runtimes.len(), 1);
    assert_eq!(runtimes[0].revision, 3);
    assert_eq!(runtimes[0].current_client_role, proto::RuntimeClientRole::Reader as i32);
    assert!(runtimes[0].has_write_owner);
    assert_eq!(runtimes[0].read_only_client_count, 1);
}

#[tokio::test]
async fn terminate_runtime_notifies_other_attached_clients_and_removes_state() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;

    writer
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
                name: "terminate-runtime".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match writer.recv().await.msg {
        Some(proto::server_message::Msg::RuntimeCreated(created)) => created.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    writer
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
                runtime_id: runtime_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    assert!(matches!(writer.recv().await.msg, Some(proto::server_message::Msg::Snapshot(_))));

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    reader
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
                runtime_id: runtime_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadOnly as i32,
            })),
        })
        .await;
    assert!(matches!(reader.recv().await.msg, Some(proto::server_message::Msg::Snapshot(_))));

    writer
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::TerminateRuntime(proto::TerminateRuntime {
                runtime_id: runtime_id.clone(),
            })),
        })
        .await;
    match writer.recv().await.msg {
        Some(proto::server_message::Msg::RuntimeTerminated(terminated)) => {
            assert_eq!(terminated.runtime_id, runtime_id);
            assert_eq!(terminated.final_revision, 4);
            assert_eq!(terminated.reason, proto::RuntimeTerminationReason::Explicit as i32);
        }
        other => panic!("expected RuntimeTerminated, got {other:?}"),
    }

    match reader.recv().await.msg {
        Some(proto::server_message::Msg::RuntimeTerminated(terminated)) => {
            assert_eq!(terminated.runtime_id, runtime_id);
            assert_eq!(terminated.final_revision, 4);
            assert_eq!(terminated.reason, proto::RuntimeTerminationReason::Explicit as i32);
        }
        other => panic!("expected pushed RuntimeTerminated, got {other:?}"),
    }

    let mut third = TestClient::connect(&sock).await;
    third.handshake().await;
    let runtimes = list_runtimes(&mut third).await;
    assert!(runtimes.is_empty());
}

#[tokio::test]
async fn read_only_client_cannot_rename_runtime() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;

    let runtime_id =
        common::create_runtime(&mut writer, "rename-denied", proto::RuntimePolicy::Persistent)
            .await;
    common::attach_rw(&mut writer, &runtime_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    common::attach_ro(&mut reader, &runtime_id).await;

    reader
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::RenameRuntime(proto::RenameRuntime {
                runtime_id: runtime_id.clone(),
                name: "hijacked".into(),
            })),
        })
        .await;
    match reader.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, 9); // ERR_OWNERSHIP_CONFLICT
        }
        other => panic!("expected Error, got {other:?}"),
    }

    // Verify name unchanged.
    let runtimes = list_runtimes(&mut writer).await;
    assert_eq!(runtimes[0].name, "rename-denied");
}

#[tokio::test]
async fn read_only_client_cannot_set_pane_title() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;

    let runtime_id =
        common::create_runtime(&mut writer, "title-denied", proto::RuntimePolicy::Persistent).await;
    common::attach_rw(&mut writer, &runtime_id).await;
    let pane_id = common::create_pane(&mut writer, &runtime_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    common::attach_ro(&mut reader, &runtime_id).await;

    reader
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::SetPaneTitle(proto::SetPaneTitle {
                runtime_id: runtime_id.clone(),
                pane_id,
                title: "hijacked".into(),
            })),
        })
        .await;
    match reader.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, 9); // ERR_OWNERSHIP_CONFLICT
        }
        other => panic!("expected Error, got {other:?}"),
    }
}
