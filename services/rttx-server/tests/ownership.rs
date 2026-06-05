//! Integration tests for runtime ownership and single-writer attach semantics.

mod common;

use common::{TestClient, list_runtimes, start_test_server};
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
            command: Some(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
                name: "writer-conflict".into(),
                policy: v3::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match writer.recv().await.payload {
        Some(v3::server_envelope::Payload::RuntimeCreated(created)) => created.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match writer.recv().await.payload {
        Some(v3::server_envelope::Payload::RuntimeSnapshot(snapshot)) => {
            assert_eq!(snapshot.client_role, v3::RuntimeClientRole::Writer as i32);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    let mut second = TestClient::connect(&sock).await;
    second.handshake().await;
    second
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match second.recv().await.payload {
        Some(v3::server_envelope::Payload::AttachBlocked(blocked)) => {
            assert_eq!(blocked.runtime_id, runtime_id);
            assert_eq!(blocked.current_client_role, v3::RuntimeClientRole::Unattached as i32);
            assert_eq!(blocked.read_only_client_count, 0);
        }
        other => panic!("expected AttachBlocked, got {other:?}"),
    }

    let runtimes = list_runtimes(&mut second).await;
    assert_eq!(runtimes.len(), 1);
    assert_eq!(runtimes[0].current_client_role, v3::RuntimeClientRole::Unattached as i32);
    assert!(runtimes[0].has_write_owner);
    assert_eq!(runtimes[0].read_only_client_count, 0);
}

#[tokio::test]
async fn read_only_attach_cannot_mutate_runtime() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;

    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
                name: "reader-denied".into(),
                policy: v3::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match writer.recv().await.payload {
        Some(v3::server_envelope::Payload::RuntimeCreated(created)) => created.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match writer.recv().await.payload {
        Some(v3::server_envelope::Payload::RuntimeSnapshot(snapshot)) => {
            assert_eq!(snapshot.runtime_revision, 2);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    reader
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::RuntimeAttachMode::ReadOnly as i32,
            })),
        })
        .await;
    match reader.recv().await.payload {
        Some(v3::server_envelope::Payload::RuntimeSnapshot(snapshot)) => {
            assert_eq!(snapshot.runtime_revision, 3);
            assert_eq!(snapshot.client_role, v3::RuntimeClientRole::Reader as i32);
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

    let runtimes = list_runtimes(&mut reader).await;
    assert_eq!(runtimes.len(), 1);
    assert_eq!(runtimes[0].runtime_revision, 3);
    assert_eq!(runtimes[0].current_client_role, v3::RuntimeClientRole::Reader as i32);
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
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
                name: "terminate-runtime".into(),
                policy: v3::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match writer.recv().await.payload {
        Some(v3::server_envelope::Payload::RuntimeCreated(created)) => created.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    assert!(matches!(
        writer.recv().await.payload,
        Some(v3::server_envelope::Payload::RuntimeSnapshot(_))
    ));

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    reader
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::RuntimeAttachMode::ReadOnly as i32,
            })),
        })
        .await;
    assert!(matches!(
        reader.recv().await.payload,
        Some(v3::server_envelope::Payload::RuntimeSnapshot(_))
    ));

    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::TerminateRuntime(v3::TerminateRuntime {
                runtime_id: runtime_id.clone(),
            })),
        })
        .await;
    match writer.recv().await.payload {
        Some(v3::server_envelope::Payload::RuntimeTerminated(terminated)) => {
            assert_eq!(terminated.runtime_id, runtime_id);
            assert_eq!(terminated.final_revision, 4);
            assert_eq!(terminated.reason, v3::RuntimeTerminationReason::Explicit as i32);
        }
        other => panic!("expected RuntimeTerminated, got {other:?}"),
    }

    match reader.recv().await.payload {
        Some(v3::server_envelope::Payload::RuntimeTerminated(terminated)) => {
            assert_eq!(terminated.runtime_id, runtime_id);
            assert_eq!(terminated.final_revision, 4);
            assert_eq!(terminated.reason, v3::RuntimeTerminationReason::Explicit as i32);
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
        common::create_runtime(&mut writer, "rename-denied", v3::RuntimePolicy::Persistent).await;
    common::attach_rw(&mut writer, &runtime_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    common::attach_ro(&mut reader, &runtime_id).await;

    reader
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::RenameRuntime(v3::RenameRuntime {
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
        common::create_runtime(&mut writer, "title-denied", v3::RuntimePolicy::Persistent).await;
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
