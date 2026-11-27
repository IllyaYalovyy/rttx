//! Integration tests for VTE parsing outside the server mutex (issue #823).
//!
//! Parsing happens under the per-workspace lock: CWD and title changes still
//! propagate, and no reader of the workspace sees a half-processed screen.

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

/// A snapshot requested while the daemon is parsing a pane's output must
/// still hold the screen: a client attaching, or a snapshot being persisted
/// for the next restart, must never see a blank one.
// Multi-threaded, like the daemon: a snapshot request has to be able to run
// while another thread is parsing.
#[tokio::test(flavor = "multi_thread")]
async fn snapshot_taken_during_output_parsing_is_never_blank() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    let (runtime_id, pane_id) = setup_attached_pane(&mut client).await;
    // Keep the parser busy with output that leaves the marker where it is.
    let cmd = "echo ON_$((40+2))SCREEN; codes=$(printf '\\033[0m%.0s' $(seq 4000)); \
               while :; do printf %s \"$codes\"; done\n";
    send_input(&mut client, &runtime_id, &pane_id, cmd.as_bytes()).await;
    let mut output = Vec::new();
    client
        .recv_matching(|payload| {
            if let v3::server_envelope::Payload::OutputDelta(delta) = payload {
                output.extend_from_slice(&delta.data);
            }
            String::from_utf8_lossy(&output).contains("ON_42SCREEN")
        })
        .await;
    common::detach_workspace(&mut client, &runtime_id).await;

    let mut observer = TestClient::connect(&sock).await;
    observer.handshake().await;
    for attempt in 0..100 {
        let snapshot = common::resync(&mut observer, &runtime_id).await;
        let screen = String::from_utf8_lossy(&snapshot.panes[0].scrollback_tail);
        assert!(screen.contains("ON_42SCREEN"), "snapshot {attempt} lost the screen: {screen:?}");
    }
    common::terminate_workspace(&mut observer, &runtime_id).await;
}
