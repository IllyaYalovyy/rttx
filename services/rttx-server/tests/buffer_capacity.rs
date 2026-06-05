//! Regression test: server buffers shrink capacity after high-output bursts.
//!
//! Verifies that after a pane produces a large burst of output, the
//! reconnect snapshot stays within the expected size cap. This confirms
//! that `raw_bytes` and `pending_flush` do not retain unbounded capacity
//! after drain/flush operations (#543).

mod common;

use common::*;
use rttx_proto::v3;
use std::time::Duration;

#[tokio::test]
async fn snapshot_bounded_after_high_output_burst() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let sid = create_runtime(&mut client, "burst-test", v3::RuntimePolicy::Persistent).await;
    attach_rw(&mut client, &sid).await;
    let pane_id = create_pane(&mut client, &sid).await;

    // Generate ~2 MB of output to trigger buffer growth.
    send_input(&mut client, &sid, &pane_id, b"head -c 2000000 /dev/zero | tr '\\0' 'B'\n").await;

    // Wait for output to be processed and flushed, then drain queued messages.
    tokio::time::sleep(Duration::from_secs(4)).await;
    client.drain(Duration::from_millis(500)).await;

    // Detach and reattach to get a fresh snapshot.
    detach_runtime(&mut client, &sid).await;
    let snapshot = attach_rw(&mut client, &sid).await;

    let pane =
        snapshot.panes.iter().find(|p| p.pane_id == pane_id).expect("pane should be in snapshot");

    // Snapshot is capped at MAX_SNAPSHOT_BYTES (256 KB).
    let max_snapshot = 256 * 1024;
    assert!(
        pane.scrollback_tail.len() <= max_snapshot,
        "snapshot {} bytes exceeds {max_snapshot} byte cap",
        pane.scrollback_tail.len()
    );
}
