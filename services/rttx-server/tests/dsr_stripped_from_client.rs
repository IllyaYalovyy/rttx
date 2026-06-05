//! Integration test: DSR/DA query sequences are stripped from client-bound output.
//!
//! Regression test for #582: applications that send DSR queries produce visible
//! `;1R` garbage because VTE generates duplicate CPR responses.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::v3;
use std::time::Duration;

async fn setup_attached_pane(client: &mut TestClient) -> (Vec<u8>, Vec<u8>) {
    client.handshake().await;

    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
            name: "strip-test".into(),
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

/// Verify that DSR query sequences do not appear in Delta messages sent to clients.
#[tokio::test]
async fn dsr_queries_stripped_from_delta_output() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let (runtime_id, pane_id) = setup_attached_pane(&mut client).await;

    client.drain(Duration::from_millis(500)).await;

    // Send a command that writes multiple DSR queries to stdout interleaved
    // with a known marker. The marker must appear in the Delta output, but
    // the raw ESC[6n sequences must not.
    let script = r"printf 'MARKER_A\033[6n\033[6nMARKER_B\033[c\033[>c'";
    let input = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id: runtime_id.clone(),
            pane_id: pane_id.clone(),
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                data: bytes::Bytes::from(format!("{script}\n").into_bytes()),
            })),
        })),
    };
    client.send(&input).await;

    let msgs = client.drain(Duration::from_secs(5)).await;
    let output: Vec<u8> = msgs
        .iter()
        .filter_map(|m| match &m.payload {
            Some(v3::server_envelope::Payload::OutputDelta(d)) => Some(d.data.to_vec()),
            _ => None,
        })
        .flatten()
        .collect();

    // The raw ESC[6n (DSR cursor position query) must not appear in client output.
    assert!(
        !output.windows(4).any(|w| w == b"\x1b[6n"),
        "DSR cursor position query must be stripped from client output"
    );

    // The raw ESC[c (DA1 query) must not appear in client output.
    let has_da1_query = output.windows(3).enumerate().any(|(i, w)| {
        w == b"\x1b[c"
            && output.get(i.wrapping_sub(1)).is_none_or(|&b| b != b'?')
            && output.get(i + 3).is_none_or(|&b| b != b'?')
    });
    assert!(!has_da1_query, "DA1 query must be stripped from client output");

    // The markers should still be present (they are plain text).
    let output_str = String::from_utf8_lossy(&output);
    assert!(
        output_str.contains("MARKER_A") && output_str.contains("MARKER_B"),
        "markers must pass through: {output_str}"
    );
}
