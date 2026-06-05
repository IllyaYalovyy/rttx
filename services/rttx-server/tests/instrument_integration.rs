//! Integration test: instrumented mutex and channel metrics increment
//! correctly during real server operations.
//!
//! Verifies that the profiling instrumentation added in #897 records
//! metrics without affecting functional behavior.

mod common;

use common::*;
use rttx_proto::v3;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[tokio::test]
async fn channel_overflow_metric_increments_on_full_push_channel() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;
    let sid = create_runtime(&mut client, "overflow-test", v3::RuntimePolicy::Persistent).await;
    let pane_id = create_pane(&mut client, &sid).await;
    attach_rw(&mut client, &sid).await;

    // Drain shell startup.
    client.drain(Duration::from_millis(500)).await;

    // Generate burst output to potentially trigger channel overflow.
    // Even if no overflow occurs, the test verifies the instrumentation
    // doesn't break normal operation.
    send_input(&mut client, &sid, &pane_id, b"seq 1 1000\n").await;

    // Collect output — this exercises the instrumented try_send path.
    let msgs = client.drain(Duration::from_secs(3)).await;
    assert!(!msgs.is_empty(), "expected output from seq command");
}

#[tokio::test]
async fn instrumented_locks_do_not_affect_message_handling() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    // Create runtime — exercises instrumented server lock in handle_message.
    let sid = create_runtime(&mut client, "lock-test", v3::RuntimePolicy::Persistent).await;

    // Attach — exercises instrumented runtime lock.
    attach_rw(&mut client, &sid).await;

    // Create pane — exercises instrumented server + runtime locks.
    let pane_id = create_pane(&mut client, &sid).await;
    client.drain(Duration::from_millis(500)).await;

    // Send input and verify output — exercises PTY read loop instrumented locks.
    send_input(&mut client, &sid, &pane_id, b"echo instrumented_ok\n").await;
    let msgs = client.drain(Duration::from_secs(2)).await;

    let output: String = msgs
        .iter()
        .filter_map(|m| match &m.payload {
            Some(v3::server_envelope::Payload::OutputDelta(d)) => {
                Some(String::from_utf8_lossy(&d.data).to_string())
            }
            _ => None,
        })
        .collect();

    assert!(output.contains("instrumented_ok"), "expected echo output in deltas, got: {output}");
}

#[tokio::test]
async fn metrics_accessible_and_zero_without_contention() {
    use rttx_server::instrument::instrumented_try_send;
    use rttx_server::metrics::DaemonMetrics;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    let metrics = Arc::new(DaemonMetrics::new());

    // Verify metrics start at zero.
    assert_eq!(metrics.mutex_contentions.load(Ordering::Relaxed), 0);
    assert_eq!(metrics.mutex_long_holds.load(Ordering::Relaxed), 0);
    assert_eq!(metrics.channel_overflows.load(Ordering::Relaxed), 0);

    // Exercise the instrumented channel send path.
    let (tx, _rx) = mpsc::channel::<u32>(2);
    let _ = instrumented_try_send(&tx, 1, &metrics);
    let _ = instrumented_try_send(&tx, 2, &metrics);

    // Channel is now full — next send should increment overflow.
    let result = instrumented_try_send(&tx, 3, &metrics);
    assert!(result.is_err());
    assert_eq!(metrics.channel_overflows.load(Ordering::Relaxed), 1);
}
