mod common;

use common::{TestClient, create_pane_with_cwd, start_test_server};
use rttx_proto::proto;
use std::time::Duration;

/// Pane created with a CWD should spawn its shell in that directory. #297.
#[tokio::test]
async fn create_pane_with_cwd_spawns_in_target_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
            name: "cwd-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let session_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::SessionCreated(sc)) => sc.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    let target_dir = std::env::temp_dir();
    let canonical_target =
        std::fs::canonicalize(&target_dir).unwrap_or_else(|_| target_dir.clone());
    let target_str = canonical_target.to_string_lossy().to_string();

    let pane_id = create_pane_with_cwd(&mut client, &session_id, Some(target_str.clone())).await;

    // Attach to receive output.
    let attach = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
            session_id: session_id.clone(),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;

    // Drain the snapshot.
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::Snapshot(_)) => break,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }
    }

    // Send `pwd` and read output.
    let input = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Input(proto::Input {
            session_id: session_id.clone(),
            pane_id: pane_id.clone(),
            data: bytes::Bytes::from_static(b"pwd\n"),
        })),
    };
    client.send(&input).await;

    // Collect output until we see the target directory.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut output = String::new();
    while tokio::time::Instant::now() < deadline {
        if let Some(msg) = client.try_recv(Duration::from_millis(200)).await
            && let Some(proto::server_message::Msg::Delta(delta)) = msg.msg
        {
            output.push_str(&String::from_utf8_lossy(&delta.data));
            if output.contains(&target_str) {
                return; // Success
            }
        }
    }
    panic!(
        "pwd output did not contain target directory {target_str:?} within timeout.\nOutput: {output}"
    );
}

/// Pane created without CWD should still work (spawns in default directory). #297.
#[tokio::test]
async fn create_pane_without_cwd_uses_default() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
            name: "no-cwd-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let session_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::SessionCreated(sc)) => sc.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    // Should not panic — None CWD is valid.
    let _pane_id = create_pane_with_cwd(&mut client, &session_id, None).await;
}
