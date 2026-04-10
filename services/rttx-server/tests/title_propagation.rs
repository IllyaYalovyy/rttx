//! Integration test: `TitleChanged` is broadcast when a pane's title changes via OSC.

mod common;

use common::{TestClient, send_input, start_test_server};
use rttx_proto::proto;
use std::time::Duration;

/// Helper: create session, pane, attach, return IDs.
async fn setup_attached_pane(client: &mut TestClient) -> (Vec<u8>, Vec<u8>) {
    client.handshake().await;

    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
            name: "title-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let session_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::SessionCreated(sc)) => sc.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    let create_pane = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
            session_id: session_id.clone(),
            cwd: None,
            dark_background: None,
        })),
    };
    client.send(&create_pane).await;
    let pane_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    let attach = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
            session_id: session_id.clone(),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::Snapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }

    (session_id, pane_id)
}

#[tokio::test]
async fn osc0_triggers_title_changed_broadcast() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    let (session_id, pane_id) = setup_attached_pane(&mut client).await;

    // Send a printf that emits an OSC 0 (set window title) escape sequence.
    let title = "rttx-title-test-42";
    let osc0_cmd = format!("printf '\\033]0;{title}\\007'\n");
    send_input(&mut client, &session_id, &pane_id, osc0_cmd.as_bytes()).await;

    // Collect messages — we should see a TitleChanged with our title among them.
    let msgs = client.drain(Duration::from_secs(5)).await;
    let title_msg = msgs.iter().find_map(|m| match &m.msg {
        Some(proto::server_message::Msg::TitleChanged(t)) if t.title == title => Some(t),
        _ => None,
    });

    assert!(
        title_msg.is_some(),
        "expected TitleChanged with title '{title}', titles seen: {:?}",
        msgs.iter()
            .filter_map(|m| match &m.msg {
                Some(proto::server_message::Msg::TitleChanged(t)) => Some(&t.title),
                _ => None,
            })
            .collect::<Vec<_>>()
    );
}
