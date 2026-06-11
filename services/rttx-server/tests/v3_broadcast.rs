//! Integration test for the v3-native broadcast pipeline.
//!
//! Regression for #980: after removing the v2 protocol, the daemon builds
//! broadcast events (`OutputDelta`, etc.) as v3 `ServerEnvelope`s directly,
//! without any v2-to-v3 conversion. This verifies that PTY output reaches an
//! attached client as a v3 `OutputDelta` carrying a monotonic
//! `pane_output_seq`.

mod common;

use common::{TestClient, attach_rw, create_pane, create_workspace, send_input, start_test_server};
use rttx_proto::v3;
use std::time::Duration;

#[tokio::test]
async fn pty_output_is_delivered_as_v3_output_delta() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;
    let runtime_id =
        create_workspace(&mut client, "v3-broadcast", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &runtime_id).await;
    let pane_id = create_pane(&mut client, &runtime_id).await;

    // Drain the shell's startup output.
    let _ = client.drain(Duration::from_millis(500)).await;

    // Produce deterministic output.
    send_input(&mut client, &runtime_id, &pane_id, b"echo v3-broadcast-marker\n").await;

    // The output must arrive as a v3 OutputDelta with a non-zero sequence.
    let env =
        client.recv_matching(|p| matches!(p, v3::server_envelope::Payload::OutputDelta(_))).await;
    let Some(v3::server_envelope::Payload::OutputDelta(delta)) = env.payload else {
        unreachable!("recv_matching guaranteed OutputDelta");
    };
    assert_eq!(delta.pane_id, pane_id, "delta must target the created pane");
    assert!(delta.pane_output_seq > 0, "v3 OutputDelta must carry a monotonic sequence");
    // Push events are not request replies.
    assert_eq!(env.request_id, 0, "broadcast push events use request_id 0");
}

#[tokio::test]
async fn output_delta_is_broadcast_to_multiple_clients() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    // Writer creates the workspace and a pane.
    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let runtime_id = create_workspace(&mut writer, "v3-fanout", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut writer, &runtime_id).await;
    let pane_id = create_pane(&mut writer, &runtime_id).await;

    // A second client attaches read-only.
    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    common::attach_ro(&mut reader, &runtime_id).await;

    let _ = writer.drain(Duration::from_millis(500)).await;
    let _ = reader.drain(Duration::from_millis(500)).await;

    send_input(&mut writer, &runtime_id, &pane_id, b"echo fanout\n").await;

    // Both clients receive the output as a v3 OutputDelta.
    for client in [&mut writer, &mut reader] {
        let env = client
            .recv_matching(|p| matches!(p, v3::server_envelope::Payload::OutputDelta(_)))
            .await;
        assert!(matches!(env.payload, Some(v3::server_envelope::Payload::OutputDelta(_))));
    }
}
