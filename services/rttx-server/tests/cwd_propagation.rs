//! Integration test: `CwdChanged` is broadcast when a pane's CWD changes.

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
            name: "cwd-test".into(),
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
async fn osc7_triggers_cwd_changed_broadcast() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    let (runtime_id, pane_id) = setup_attached_pane(&mut client).await;

    // cd + manual OSC 7 emission + hold with cat. The manual printf
    // ensures the test works even without /etc/profile.d/vte-2.91.sh
    // (e.g., in CI containers). `cat` prevents PROMPT_COMMAND from
    // overwriting the CWD on the next prompt.
    let target = "/tmp";
    let cmd = format!("cd {target} && printf '\\033]7;file://localhost{target}\\033\\\\'; cat\n");
    send_input(&mut client, &runtime_id, &pane_id, cmd.as_bytes()).await;

    // Collect messages — we should see a CwdChanged with /tmp.
    let msgs = client.drain(Duration::from_secs(5)).await;
    let cwd_msg = msgs.iter().find_map(|m| match &m.payload {
        Some(v3::server_envelope::Payload::CwdChanged(c)) if c.cwd == target => Some(c),
        _ => None,
    });

    assert!(cwd_msg.is_some(), "expected CwdChanged with path {target}");
}
