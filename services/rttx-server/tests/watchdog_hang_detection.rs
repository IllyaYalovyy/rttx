//! Integration test: watchdog detects daemon hang and generates report.
//!
//! Holds the server mutex for longer than the watchdog threshold and
//! verifies that `hang-report.txt` is generated with a metrics snapshot.

mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use rttx_server::flight::{EventType, RingReader, RingWriter, SpanKind};
use rttx_server::metrics::DaemonMetrics;
use rttx_server::server::Server;
use rttx_server::watchdog::{WatchdogConfig, spawn_watchdog_with_config};
use tempfile::TempDir;
use tokio::sync::{Mutex, watch};

fn create_test_server() -> Server {
    use rttx_server::os::OsInterface;
    use std::path::PathBuf;

    #[derive(Debug)]
    struct TestOs;
    impl OsInterface for TestOs {
        fn runtime_dir(&self) -> PathBuf {
            PathBuf::from("/tmp/test-runtime")
        }
        fn cache_dir(&self) -> PathBuf {
            PathBuf::from("/tmp/test-cache")
        }
        fn state_dir(&self) -> PathBuf {
            PathBuf::from("/tmp/test-state/rttx/daemon")
        }
    }
    let dir = tempfile::TempDir::new().unwrap();
    let ring = Arc::new(rttx_server::flight::RingWriter::open(dir.path()).unwrap());
    std::mem::forget(dir);
    Server::new(Box::new(TestOs), Arc::new(DaemonMetrics::new()), ring)
}

#[tokio::test]
async fn watchdog_generates_hang_report_on_prolonged_mutex_hold() {
    let dir = TempDir::new().unwrap();
    let metrics = Arc::new(DaemonMetrics::new());
    metrics.connected_clients.store(2, Ordering::Relaxed);
    metrics.active_panes.store(4, Ordering::Relaxed);
    metrics.total_channel_depth.store(50, Ordering::Relaxed);

    let ring = Arc::new(RingWriter::open(dir.path()).unwrap());
    let server = Arc::new(Mutex::new(create_test_server()));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let epoch = Instant::now();

    // Use fast config: 50ms interval, 25ms timeout, 3 max.
    // Total time to trigger: 3 × (50ms + 25ms) = 225ms.
    let config = WatchdogConfig {
        check_interval: Duration::from_millis(50),
        timeout: Duration::from_millis(25),
        max_consecutive_timeouts: 3,
    };

    // Hold the mutex to simulate a hang.
    let guard = server.lock().await;

    let state = spawn_watchdog_with_config(
        Arc::clone(&server),
        Arc::clone(&metrics),
        Arc::clone(&ring),
        dir.path().to_path_buf(),
        shutdown_rx,
        epoch,
        &config,
    );

    // Wait long enough for 3+ timeouts.
    tokio::time::sleep(Duration::from_millis(400)).await;

    // Verify the watchdog detected the hang.
    assert!(
        state.consecutive_timeouts.load(Ordering::Relaxed) >= 3,
        "expected at least 3 consecutive timeouts"
    );

    // Release the mutex and stop the watchdog.
    drop(guard);
    let _ = shutdown_tx.send(true);
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Verify hang-report.txt was generated.
    let report_path = dir.path().join("hang-report.txt");
    assert!(report_path.exists(), "hang-report.txt should be generated");

    let content = std::fs::read_to_string(&report_path).unwrap();
    assert!(content.contains("rttx-server hang report"));
    assert!(content.contains("Connected clients: 2"));
    assert!(content.contains("Active panes: 4"));
    assert!(content.contains("Channel depth: 50"));
    assert!(content.contains("Latency histograms"));

    // Verify ring buffer contains WatchdogTimeout events.
    let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
    let events = reader.read_all();
    let timeout_events: Vec<_> = events
        .iter()
        .filter(|e| e.span_kind == SpanKind::WatchdogTimeout && e.event_type == EventType::Event)
        .collect();
    assert!(
        timeout_events.len() >= 3,
        "expected at least 3 WatchdogTimeout events, got {}",
        timeout_events.len()
    );

    // Verify the value field contains the consecutive count.
    assert_eq!(timeout_events[0].value, 1);
    assert_eq!(timeout_events[1].value, 2);
    assert_eq!(timeout_events[2].value, 3);
}

#[tokio::test]
async fn watchdog_recovers_after_mutex_released() {
    let dir = TempDir::new().unwrap();
    let metrics = Arc::new(DaemonMetrics::new());
    let ring = Arc::new(RingWriter::open(dir.path()).unwrap());
    let server = Arc::new(Mutex::new(create_test_server()));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let epoch = Instant::now();

    let config = WatchdogConfig {
        check_interval: Duration::from_millis(50),
        timeout: Duration::from_millis(25),
        max_consecutive_timeouts: 3,
    };

    // Hold the mutex briefly to cause 1-2 timeouts.
    let guard = server.lock().await;

    let state = spawn_watchdog_with_config(
        Arc::clone(&server),
        Arc::clone(&metrics),
        Arc::clone(&ring),
        dir.path().to_path_buf(),
        shutdown_rx,
        epoch,
        &config,
    );

    // Wait for 1 timeout.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(state.consecutive_timeouts.load(Ordering::Relaxed) >= 1);

    // Release the mutex.
    drop(guard);

    // Wait for the watchdog to successfully acquire and reset.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        state.consecutive_timeouts.load(Ordering::Relaxed),
        0,
        "counter should reset after successful acquire"
    );

    let _ = shutdown_tx.send(true);
}
