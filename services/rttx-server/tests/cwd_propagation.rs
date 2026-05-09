//! Integration test: `CwdChanged` is broadcast when a pane's CWD changes.

mod common;

use common::{TestClient, send_input, start_test_server};
use rttx_proto::proto;
use std::time::Duration;

/// Helper: create session, pane, attach, return IDs.
async fn setup_attached_pane(client: &mut TestClient) -> (Vec<u8>, Vec<u8>) {
    client.handshake().await;

    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "cwd-test".into(),
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

#[tokio::test]
async fn osc7_triggers_cwd_changed_broadcast() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    let (runtime_id, pane_id) = setup_attached_pane(&mut client).await;

    // Send a printf that emits an OSC 7 escape sequence.
    // Use `cd` so the shell's PROMPT_COMMAND emits OSC 7 with the real CWD.
    // A raw printf of OSC 7 gets immediately overwritten by the next prompt.
    let target = "/tmp";
    send_input(&mut client, &runtime_id, &pane_id, format!("cd {target}\n").as_bytes()).await;

    // Collect messages — we should see a CwdChanged with /tmp.
    let msgs = client.drain(Duration::from_secs(5)).await;
    let cwd_msg = msgs.iter().rev().find_map(|m| match &m.msg {
        Some(proto::server_message::Msg::CwdChanged(c)) if c.cwd == target => Some(c),
        _ => None,
    });

    assert!(cwd_msg.is_some(), "expected CwdChanged with path {target}");
}
