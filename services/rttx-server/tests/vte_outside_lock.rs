//! Integration test: VTE parsing outside the mutex still propagates
//! CWD and title changes correctly.
//!
//! Verifies that the two-phase `accept_output`/`parse_and_extract` split
//! (issue #823) does not regress metadata extraction.

mod common;

use common::{TestClient, send_input, start_test_server};
use rttx_proto::v3;
use std::time::Duration;

async fn setup_attached_pane(client: &mut TestClient) -> (Vec<u8>, Vec<u8>) {
    client.handshake().await;
    let runtime_id =
        common::create_workspace(client, "vte-test", v3::WorkspacePolicy::Persistent).await;
    let pane_id = common::create_pane(client, &runtime_id).await;
    common::attach_rw(client, &runtime_id).await;
    (runtime_id, pane_id)
}

#[tokio::test]
async fn title_and_cwd_propagated_after_two_phase_parsing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    let (runtime_id, pane_id) = setup_attached_pane(&mut client).await;

    // Emit OSC 0 (title) and OSC 7 (CWD) in a single printf, then hold
    // with `cat` to prevent PROMPT_COMMAND from overwriting them.
    let cmd = "printf '\\033]0;my-project\\007\\033]7;file://localhost/tmp/project\\007'; cat\n";
    send_input(&mut client, &runtime_id, &pane_id, cmd.as_bytes()).await;

    let msgs = client.drain(Duration::from_secs(5)).await;

    let title_msg = msgs.iter().find_map(|m| match &m.payload {
        Some(v3::server_envelope::Payload::TitleChanged(t)) if t.title == "my-project" => {
            Some(t.title.clone())
        }
        _ => None,
    });
    let cwd_msg = msgs.iter().find_map(|m| match &m.payload {
        Some(v3::server_envelope::Payload::CwdChanged(c)) if c.cwd == "/tmp/project" => {
            Some(c.cwd.clone())
        }
        _ => None,
    });

    assert_eq!(title_msg.as_deref(), Some("my-project"), "title should propagate");
    assert_eq!(cwd_msg.as_deref(), Some("/tmp/project"), "CWD should propagate");
}
