//! Integration tests for session lifecycle.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::proto;
use std::time::Duration;

#[tokio::test]
async fn create_runtime_and_list() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    // Create a session.
    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "test-session".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let resp = client.recv().await;
    let runtime_id = match resp.msg {
        Some(proto::server_message::Msg::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };
    assert_eq!(runtime_id.len(), 16);

    // List runtimes.
    let list = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ListRuntimes(proto::ListRuntimes {})),
    };
    client.send(&list).await;
    let resp = client.recv().await;
    match resp.msg {
        Some(proto::server_message::Msg::RuntimeList(sl)) => {
            assert_eq!(sl.runtimes.len(), 1);
            assert_eq!(sl.runtimes[0].name, "test-session");
        }
        other => panic!("expected RuntimeList, got {other:?}"),
    }
}

#[tokio::test]
async fn attach_and_detach_runtime() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    // Create session.
    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "attach-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let resp = client.recv().await;
    let runtime_id = match resp.msg {
        Some(proto::server_message::Msg::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    // Attach.
    let attach = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
            runtime_id: runtime_id.clone(),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    let resp = client.recv().await;
    match resp.msg {
        Some(proto::server_message::Msg::Snapshot(snap)) => {
            assert_eq!(snap.runtime_id, runtime_id);
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    // Detach.
    let detach = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::DetachRuntime(proto::DetachRuntime {
            runtime_id: runtime_id.clone(),
        })),
    };
    client.send(&detach).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for RuntimeDetached");
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::RuntimeDetached(_)) => break,
            Some(
                proto::server_message::Msg::Delta(_) | proto::server_message::Msg::PaneExited(_),
            ) => {}
            other => panic!("expected RuntimeDetached, got {other:?}"),
        }
    }

    // Verify session still exists after detach.
    let list = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ListRuntimes(proto::ListRuntimes {})),
    };
    client.send(&list).await;
    let resp = client.recv().await;
    match resp.msg {
        Some(proto::server_message::Msg::RuntimeList(sl)) => {
            assert_eq!(sl.runtimes.len(), 1);
        }
        other => panic!("expected RuntimeList, got {other:?}"),
    }
}

#[tokio::test]
async fn create_and_close_pane() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    // Create session.
    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "pane-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let resp = client.recv().await;
    let runtime_id = match resp.msg {
        Some(proto::server_message::Msg::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    // Create pane.
    let create_pane = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
            runtime_id: runtime_id.clone(),
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
        })),
    };
    client.send(&create_pane).await;
    let resp = client.recv().await;
    let pane_id = match resp.msg {
        Some(proto::server_message::Msg::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    // Close pane.
    let close_pane = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
            runtime_id: runtime_id.clone(),
            pane_id,
        })),
    };
    client.send(&close_pane).await;
    let resp = client.recv().await;
    assert!(matches!(resp.msg, Some(proto::server_message::Msg::PaneClosed(_))));
}

#[tokio::test]
async fn rename_runtime_updates_name_and_inventory() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let runtime_id =
        common::create_runtime(&mut client, "original", proto::RuntimePolicy::Persistent).await;
    common::attach_rw(&mut client, &runtime_id).await;

    // Rename the session.
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::RenameRuntime(proto::RenameRuntime {
                runtime_id: runtime_id.clone(),
                name: "renamed".into(),
            })),
        })
        .await;

    // Expect RuntimeRenamed response.
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::RuntimeRenamed(renamed)) => {
                assert_eq!(renamed.runtime_id, runtime_id);
                assert_eq!(renamed.name, "renamed");
                break;
            }
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected RuntimeRenamed, got {other:?}"),
        }
    }

    // Verify inventory reflects the new name.
    let runtimes = common::list_runtimes(&mut client).await;
    assert_eq!(runtimes.len(), 1);
    assert_eq!(runtimes[0].name, "renamed");
}

#[tokio::test]
async fn rename_runtime_persists_across_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let runtime_id =
        common::create_runtime(&mut client, "before", proto::RuntimePolicy::Persistent).await;
    common::attach_rw(&mut client, &runtime_id).await;

    // Rename.
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::RenameRuntime(proto::RenameRuntime {
                runtime_id: runtime_id.clone(),
                name: "after".into(),
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::RuntimeRenamed(_)) => break,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected RuntimeRenamed, got {other:?}"),
        }
    }

    // Wait for state to be persisted with the new name.
    common::wait_for_state_containing(&tmp.path().join("cache"), "after", Duration::from_secs(5))
        .await;

    // Shutdown and restart.
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Shutdown(proto::Shutdown {})),
        })
        .await;
    let _ = handle.await;

    let (socket_path2, _handle2) = start_test_server(tmp.path()).await;
    let mut client2 = TestClient::connect(&socket_path2).await;
    client2.handshake().await;

    let runtimes = common::list_runtimes(&mut client2).await;
    assert_eq!(runtimes.len(), 1);
    assert_eq!(runtimes[0].name, "after");
}
