//! Integration test: CWD polling via /proc/<pid>/cwd detects changes
//! even when OSC 7 is not emitted by the shell.

mod common;

use common::{TestClient, send_input, start_test_server};
use rttx_proto::proto;
use std::time::Duration;

/// Helper: create runtime, pane, attach, return IDs.
async fn setup_attached_pane(client: &mut TestClient) -> (Vec<u8>, Vec<u8>) {
    client.handshake().await;

    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "proc-poll-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let runtime_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    let create_pane = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
            runtime_id: runtime_id.clone(),
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
            no_persist: None,
        })),
    };
    client.send(&create_pane).await;
    let pane_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    let attach = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
            runtime_id: runtime_id.clone(),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::Snapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }

    (runtime_id, pane_id)
}

/// When OSC 7 is never emitted, the serialization loop's /proc poll
/// should detect the CWD change and broadcast `CwdChanged`.
#[tokio::test]
async fn proc_cwd_poll_detects_cd_without_osc7() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    let (runtime_id, pane_id) = setup_attached_pane(&mut client).await;

    // Use `cd` followed by `cat` to hold open the pane without running
    // PROMPT_COMMAND (which would emit OSC 7 if configured). The shell
    // itself changes CWD, so /proc/<pid>/cwd reflects it.
    let target = "/tmp";
    let cmd = format!("cd {target} && exec cat\n");
    send_input(&mut client, &runtime_id, &pane_id, cmd.as_bytes()).await;

    // Wait for the 5-second CWD poll interval to fire.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    let mut found = false;
    while tokio::time::Instant::now() < deadline {
        if let Some(msg) = client.try_recv(Duration::from_millis(500)).await
            && let Some(proto::server_message::Msg::CwdChanged(c)) = &msg.msg
            && c.cwd == target
        {
            found = true;
            break;
        }
    }

    assert!(found, "expected CwdChanged with path {target} from /proc poll within 12 seconds");
}
