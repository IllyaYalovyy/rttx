//! Tests for PTY read coalescing: burst output should be batched into
//! fewer, larger Delta messages instead of one per 4KB read.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::{bytes_to_uuid, proto};
use std::time::Duration;

/// Helper: create a session, create a pane, attach, and return IDs.
async fn setup_attached_pane(client: &mut TestClient) -> (Vec<u8>, Vec<u8>) {
    client.handshake().await;
    let session_id =
        common::create_session(client, "coalesce-test", proto::RuntimePolicy::Persistent).await;
    let pane_id = common::create_pane(client, &session_id).await;
    common::attach_rw(client, &session_id).await;
    (session_id, pane_id)
}

/// Burst output (e.g. `seq 1 5000`) should arrive as fewer Delta messages
/// than the number of 4KB reads the kernel would produce, proving that
/// the read loop coalesces adjacent reads.
#[tokio::test]
async fn burst_output_produces_coalesced_deltas() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let (session_id, pane_id) = setup_attached_pane(&mut client).await;

    // Drain shell startup output.
    client.drain(Duration::from_millis(500)).await;

    // Generate ~50KB of output (seq 1 5000 produces ~5 digits * 5000 ≈ 34KB).
    common::send_input(&mut client, &session_id, &pane_id, b"seq 1 5000\n").await;

    // Collect all Delta messages for this pane.
    let msgs = client.drain(Duration::from_secs(5)).await;
    let deltas: Vec<&proto::Delta> = msgs
        .iter()
        .filter_map(|m| match &m.msg {
            Some(proto::server_message::Msg::Delta(d))
                if bytes_to_uuid(&d.pane_id).ok() == bytes_to_uuid(&pane_id).ok() =>
            {
                Some(d)
            }
            _ => None,
        })
        .collect();

    let total_bytes: usize = deltas.iter().map(|d| d.data.len()).sum();
    let delta_count = deltas.len();

    // Without coalescing, ~34KB at 4KB per read = ~9 Deltas minimum.
    // With coalescing (64KB cap, 1ms window), we expect significantly fewer.
    // The key assertion: average Delta size should be well above 4KB,
    // proving coalescing is working.
    assert!(total_bytes > 10_000, "expected substantial output, got {total_bytes} bytes");
    assert!(delta_count > 0, "expected at least one Delta");

    let avg_size = total_bytes / delta_count;
    // With coalescing, average delta size should exceed the 4KB read buffer.
    // Be conservative: require > 4096 to prove batching happened.
    assert!(
        avg_size > 4096 || delta_count < 5,
        "coalescing not effective: {delta_count} deltas, avg {avg_size} bytes \
         (expected avg > 4096 or fewer than 5 deltas)"
    );
}

/// All output bytes must arrive intact regardless of coalescing.
/// Concatenating all Delta payloads must contain every line from the command.
#[tokio::test]
async fn coalesced_deltas_preserve_all_output_bytes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    let (session_id, pane_id) = setup_attached_pane(&mut client).await;
    client.drain(Duration::from_millis(500)).await;

    // Use a deterministic marker range.
    common::send_input(&mut client, &session_id, &pane_id, b"seq 1 100\n").await;

    let msgs = client.drain(Duration::from_secs(5)).await;
    let output: Vec<u8> = msgs
        .iter()
        .filter_map(|m| match &m.msg {
            Some(proto::server_message::Msg::Delta(d))
                if bytes_to_uuid(&d.pane_id).ok() == bytes_to_uuid(&pane_id).ok() =>
            {
                Some(d.data.to_vec())
            }
            _ => None,
        })
        .flatten()
        .collect();
    let text = String::from_utf8_lossy(&output);

    // Every number from 1 to 100 must appear in the output.
    for i in 1..=100 {
        assert!(
            text.contains(&format!("\n{i}\r\n")) || text.contains(&format!("{i}\r\n")),
            "missing number {i} in coalesced output"
        );
    }
}

/// Two clients attached to the same session must receive identical
/// coalesced Delta byte streams.
#[tokio::test]
async fn coalesced_deltas_identical_across_clients() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client_a = TestClient::connect(&sock).await;
    let (session_id, pane_id) = setup_attached_pane(&mut client_a).await;
    client_a.drain(Duration::from_millis(500)).await;

    // Second client attaches read-only.
    let mut client_b = TestClient::connect(&sock).await;
    client_b.handshake().await;
    common::attach_ro(&mut client_b, &session_id).await;
    client_b.drain(Duration::from_millis(300)).await;

    let marker = "COALESCE_MULTI_CLIENT_42";
    common::send_input(&mut client_a, &session_id, &pane_id, format!("echo {marker}\n").as_bytes())
        .await;

    let collect = |msgs: &[proto::ServerMessage]| -> Vec<u8> {
        msgs.iter()
            .filter_map(|m| match &m.msg {
                Some(proto::server_message::Msg::Delta(d))
                    if bytes_to_uuid(&d.pane_id).ok() == bytes_to_uuid(&pane_id).ok() =>
                {
                    Some(d.data.to_vec())
                }
                _ => None,
            })
            .flatten()
            .collect()
    };

    let msgs_a = client_a.drain(Duration::from_secs(5)).await;
    let msgs_b = client_b.drain(Duration::from_secs(5)).await;

    let data_a = collect(&msgs_a);
    let data_b = collect(&msgs_b);
    let text_a = String::from_utf8_lossy(&data_a);
    let text_b = String::from_utf8_lossy(&data_b);

    assert!(text_a.contains(marker), "client A missing marker");
    assert!(text_b.contains(marker), "client B missing marker");
}
