//! Integration test: `DaemonMetrics` shared across threads via Arc.
//!
//! Verifies that the metrics types work correctly when used as intended —
//! shared via Arc across multiple async tasks simulating real server components.

use rttx_server::metrics::{DaemonMetrics, LatencyHistogram};
use std::sync::Arc;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn metrics_shared_across_async_tasks() {
    let metrics = Arc::new(DaemonMetrics::new());

    let mut handles = Vec::new();

    // Simulate PTY reader task
    let m = Arc::clone(&metrics);
    handles.push(tokio::spawn(async move {
        for _ in 0..500 {
            m.bytes_read_from_pty.fetch_add(256, Ordering::Relaxed);
            m.pty_read_latency_us.record(45);
        }
    }));

    // Simulate client writer task
    let m = Arc::clone(&metrics);
    handles.push(tokio::spawn(async move {
        for _ in 0..500 {
            m.bytes_written_to_clients.fetch_add(128, Ordering::Relaxed);
            m.client_write_latency_us.record(200);
        }
    }));

    // Simulate dispatch task
    let m = Arc::clone(&metrics);
    handles.push(tokio::spawn(async move {
        for _ in 0..500 {
            m.messages_dispatched.fetch_add(1, Ordering::Relaxed);
            m.dispatch_latency_us.record(75);
        }
    }));

    for h in handles {
        h.await.unwrap();
    }

    assert_eq!(metrics.bytes_read_from_pty.load(Ordering::Relaxed), 500 * 256);
    assert_eq!(metrics.bytes_written_to_clients.load(Ordering::Relaxed), 500 * 128);
    assert_eq!(metrics.messages_dispatched.load(Ordering::Relaxed), 500);

    // Histogram percentiles reflect the recorded values
    assert_eq!(metrics.pty_read_latency_us.percentile(50.0), Some(100)); // [10, 100)
    assert_eq!(metrics.client_write_latency_us.percentile(50.0), Some(1_000)); // [100, 1000)
    assert_eq!(metrics.dispatch_latency_us.percentile(50.0), Some(100)); // [10, 100)
}

#[tokio::test]
async fn gauge_tracks_client_connect_disconnect() {
    let metrics = Arc::new(DaemonMetrics::new());

    // Simulate 3 clients connecting
    for _ in 0..3 {
        metrics.connected_clients.fetch_add(1, Ordering::Relaxed);
    }
    assert_eq!(metrics.connected_clients.load(Ordering::Relaxed), 3);

    // One disconnects
    metrics.connected_clients.fetch_sub(1, Ordering::Relaxed);
    metrics.client_disconnects.fetch_add(1, Ordering::Relaxed);

    assert_eq!(metrics.connected_clients.load(Ordering::Relaxed), 2);
    assert_eq!(metrics.client_disconnects.load(Ordering::Relaxed), 1);
}

#[test]
fn histogram_percentile_accuracy_across_distribution() {
    let h = LatencyHistogram::new();

    // Simulate realistic latency distribution:
    // 70% fast (< 10µs), 20% medium (10-100µs), 8% slow (100-1000µs), 2% very slow (1-10ms)
    for _ in 0..700 {
        h.record(5);
    }
    for _ in 0..200 {
        h.record(50);
    }
    for _ in 0..80 {
        h.record(500);
    }
    for _ in 0..20 {
        h.record(5_000);
    }

    // p50 should be in the fast bucket [0, 10) → upper bound 10
    assert_eq!(h.percentile(50.0), Some(10));
    // p90 should be in the medium bucket: cumulative at bucket 1 = 900 >= 900
    assert_eq!(h.percentile(90.0), Some(100));
    // p98 threshold = ceil(1000 * 98/100) = 980, cumulative at bucket 2 = 980 >= 980
    assert_eq!(h.percentile(98.0), Some(1_000));
    // p99 threshold = 990, cumulative at bucket 3 = 1000 >= 990
    assert_eq!(h.percentile(99.0), Some(10_000));
}

#[tokio::test]
async fn lock_contention_increments_mutex_contention_metric() {
    use rttx_server::instrument::lock_workspace;
    use rttx_server::workspace::Workspace;
    use tokio::sync::{Mutex, oneshot};

    let metrics = Arc::new(DaemonMetrics::new());
    let mutex = Arc::new(Mutex::new(Workspace::new("contended".to_string())));

    // Hold the lock in another task and signal once it is *actually* held, so
    // the contended acquisition below is guaranteed to wait — no reliance on
    // sleep timing to win the race (the source of earlier flakiness under
    // coverage instrumentation). The holder keeps the lock well past the 1ms
    // contention threshold.
    let (acquired_tx, acquired_rx) = oneshot::channel();
    let holder = {
        let mutex = Arc::clone(&mutex);
        tokio::spawn(async move {
            let guard = mutex.lock().await;
            let _ = acquired_tx.send(());
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            drop(guard);
        })
    };

    // Only attempt the contended acquire once the holder truly owns the lock.
    acquired_rx.await.unwrap();
    let guard = lock_workspace(&mutex, &metrics).await;
    drop(guard);
    holder.await.unwrap();

    assert!(
        metrics.mutex_contentions.load(Ordering::Relaxed) >= 1,
        "a lock wait past the contention threshold must be recorded"
    );
}
