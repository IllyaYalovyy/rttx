//! Integration test: `TitleChanged` is broadcast when a pane's title changes via OSC.

mod common;

use common::{TestClient, send_input, start_test_server};
use rttx_proto::v3;
use std::time::Duration;

/// Helper: create session, pane, attach, return IDs.
async fn setup_attached_pane(client: &mut TestClient) -> (Vec<u8>, Vec<u8>) {
    client.handshake().await;

    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
            name: "title-test".into(),
            policy: v3::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let runtime_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    let create_pane = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
            runtime_id: runtime_id.clone(),
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
            no_persist: None,
        })),
    };
    client.send(&create_pane).await;
    let pane_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    let attach = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
            runtime_id: runtime_id.clone(),
            attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::RuntimeSnapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }

    (runtime_id, pane_id)
}

#[tokio::test]
async fn osc0_triggers_title_changed_broadcast() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    let (runtime_id, pane_id) = setup_attached_pane(&mut client).await;

    // Send a printf that emits an OSC 0 (set window title) escape sequence.
    let title = "rttx-title-test-42";
    // Use `cat` to hold the shell after setting the title, preventing
    // PROMPT_COMMAND from overwriting it with the default title.
    let osc0_cmd = format!("printf '\\033]0;{title}\\007'; cat\n");
    send_input(&mut client, &runtime_id, &pane_id, osc0_cmd.as_bytes()).await;

    // Collect messages — we should see a TitleChanged with our title among them.
    let msgs = client.drain(Duration::from_secs(5)).await;
    let title_msg = msgs.iter().find_map(|m| match &m.payload {
        Some(v3::server_envelope::Payload::TitleChanged(t)) if t.title == title => Some(t),
        _ => None,
    });

    assert!(
        title_msg.is_some(),
        "expected TitleChanged with title '{title}', titles seen: {:?}",
        msgs.iter()
            .filter_map(|m| match &m.payload {
                Some(v3::server_envelope::Payload::TitleChanged(t)) => Some(&t.title),
                _ => None,
            })
            .collect::<Vec<_>>()
    );
}
