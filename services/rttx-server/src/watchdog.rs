//! Watchdog task for daemon hang detection.
//!
//! Runs independently of the main server loop. Every `check_interval`
//! seconds it attempts to acquire the server mutex with a timeout. If
//! the mutex cannot be acquired within `timeout`, a `WATCHDOG_TIMEOUT`
//! event is written to the ring buffer. After `max_consecutive_timeouts`
//! consecutive failures, a hang report is written to disk.

use std::fmt::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, watch};

use crate::flight::{EventType, FlightEvent, RingWriter, SpanKind};
use crate::metrics::DaemonMetrics;
use crate::server::Server;

/// Configuration for the watchdog task.
#[derive(Debug, Clone)]
pub struct WatchdogConfig {
    /// How often the watchdog checks the server mutex.
    pub check_interval: Duration,
    /// How long to wait for the mutex before declaring a timeout.
    pub timeout: Duration,
    /// Number of consecutive timeouts before writing a hang report.
    pub max_consecutive_timeouts: u32,
}

impl Default for WatchdogConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(5),
            timeout: Duration::from_secs(2),
            max_consecutive_timeouts: 3,
        }
    }
}

/// Shared state for the watchdog, accessible for testing.
pub struct WatchdogState {
    pub consecutive_timeouts: AtomicU32,
}

impl Default for WatchdogState {
    fn default() -> Self {
        Self::new()
    }
}

impl WatchdogState {
    #[must_use]
    pub const fn new() -> Self {
        Self { consecutive_timeouts: AtomicU32::new(0) }
    }
}

/// Spawn the watchdog task with default production configuration.
pub fn spawn_watchdog(
    server: Arc<Mutex<Server>>,
    metrics: Arc<DaemonMetrics>,
    ring: Arc<RingWriter>,
    cache_dir: PathBuf,
    shutdown_rx: watch::Receiver<bool>,
    epoch: Instant,
) -> Arc<WatchdogState> {
    spawn_watchdog_with_config(
        server,
        metrics,
        ring,
        cache_dir,
        shutdown_rx,
        epoch,
        &WatchdogConfig::default(),
    )
}

/// Spawn the watchdog task with custom configuration (for testing).
pub fn spawn_watchdog_with_config(
    server: Arc<Mutex<Server>>,
    metrics: Arc<DaemonMetrics>,
    ring: Arc<RingWriter>,
    cache_dir: PathBuf,
    mut shutdown_rx: watch::Receiver<bool>,
    epoch: Instant,
    config: &WatchdogConfig,
) -> Arc<WatchdogState> {
    let state = Arc::new(WatchdogState::new());
    let state_clone = Arc::clone(&state);
    let config = config.clone();

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(config.check_interval);
        // First tick fires immediately; skip it so the first real check
        // happens after check_interval.
        interval.tick().await;

        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = shutdown_rx.changed() => {
                    tracing::info!("Watchdog stopping (shutdown)");
                    return;
                }
            }

            let result = tokio::time::timeout(config.timeout, server.lock()).await;

            if let Ok(_guard) = result {
                state_clone.consecutive_timeouts.store(0, Ordering::Relaxed);
            } else {
                let count = state_clone.consecutive_timeouts.fetch_add(1, Ordering::Relaxed) + 1;

                // Record timeout event in ring buffer.
                ring.record(&FlightEvent {
                    timestamp_ns: epoch.elapsed().as_nanos() as u64,
                    span_id: 0,
                    event_type: EventType::Event,
                    span_kind: SpanKind::WatchdogTimeout,
                    context: [0; 16],
                    value: u64::from(count),
                });

                tracing::warn!(consecutive = count, "Watchdog: server mutex acquisition timed out");

                if count >= config.max_consecutive_timeouts {
                    let report = build_hang_report(&metrics, count);
                    let report_path = cache_dir.join("hang-report.txt");
                    if let Err(e) = write_hang_report(&report_path, &report) {
                        tracing::error!("Watchdog: failed to write hang report: {e}");
                    }
                    tracing::error!(
                        "Watchdog: daemon unresponsive for {}s",
                        u64::from(count) * config.check_interval.as_secs()
                    );
                }
            }
        }
    });

    state
}

/// Build a hang report string from current metrics.
fn build_hang_report(metrics: &DaemonMetrics, consecutive_timeouts: u32) -> String {
    let mut report = String::new();
    let _ = writeln!(report, "=== rttx-server hang report ===");
    let _ = writeln!(report, "Consecutive timeouts: {consecutive_timeouts}");
    let _ = writeln!(report);
    let _ = writeln!(report, "-- Metrics snapshot --");
    let _ = writeln!(
        report,
        "Connected clients: {}",
        metrics.connected_clients.load(Ordering::Relaxed)
    );
    let _ = writeln!(report, "Active panes: {}", metrics.active_panes.load(Ordering::Relaxed));
    let _ =
        writeln!(report, "Channel depth: {}", metrics.total_channel_depth.load(Ordering::Relaxed));
    let _ = writeln!(
        report,
        "Messages dispatched: {}",
        metrics.messages_dispatched.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        report,
        "Bytes read from PTY: {}",
        metrics.bytes_read_from_pty.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        report,
        "Bytes written to clients: {}",
        metrics.bytes_written_to_clients.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        report,
        "Channel overflows: {}",
        metrics.channel_overflows.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        report,
        "Mutex contentions: {}",
        metrics.mutex_contentions.load(Ordering::Relaxed)
    );
    let _ =
        writeln!(report, "Mutex long holds: {}", metrics.mutex_long_holds.load(Ordering::Relaxed));
    let _ = writeln!(report);
    let _ = writeln!(report, "-- Latency histograms --");
    let _ = writeln!(report, "Mutex wait (µs): {:?}", metrics.mutex_wait_us.snapshot());
    let _ = writeln!(report, "Dispatch latency (µs): {:?}", metrics.dispatch_latency_us.snapshot());
    let _ = writeln!(report, "PTY read latency (µs): {:?}", metrics.pty_read_latency_us.snapshot());
    let _ = writeln!(
        report,
        "Client write latency (µs): {:?}",
        metrics.client_write_latency_us.snapshot()
    );
    let _ =
        writeln!(report, "VTE parse latency (µs): {:?}", metrics.vte_parse_latency_us.snapshot());
    let _ = writeln!(
        report,
        "Serialization tick latency (µs): {:?}",
        metrics.serialization_tick_latency_us.snapshot()
    );
    let _ = writeln!(report, "IO flush latency (µs): {:?}", metrics.io_flush_latency_us.snapshot());
    report
}

/// Write the hang report to disk.
fn write_hang_report(path: &std::path::Path, report: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_config() -> WatchdogConfig {
        WatchdogConfig {
            check_interval: Duration::from_millis(100),
            timeout: Duration::from_millis(50),
            max_consecutive_timeouts: 3,
        }
    }

    #[test]
    fn build_hang_report_contains_metrics() {
        let metrics = DaemonMetrics::new();
        metrics.connected_clients.store(2, Ordering::Relaxed);
        metrics.active_panes.store(5, Ordering::Relaxed);
        metrics.total_channel_depth.store(100, Ordering::Relaxed);

        let report = build_hang_report(&metrics, 3);

        assert!(report.contains("Consecutive timeouts: 3"));
        assert!(report.contains("Connected clients: 2"));
        assert!(report.contains("Active panes: 5"));
        assert!(report.contains("Channel depth: 100"));
        assert!(report.contains("Mutex wait"));
    }

    #[test]
    fn write_hang_report_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("hang-report.txt");

        write_hang_report(&path, "test report content").unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "test report content");
    }

    #[test]
    fn write_hang_report_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("dir").join("hang-report.txt");

        write_hang_report(&path, "nested").unwrap();

        assert!(path.exists());
    }

    #[test]
    fn watchdog_state_initial_value_is_zero() {
        let state = WatchdogState::new();
        assert_eq!(state.consecutive_timeouts.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn default_config_has_production_values() {
        let config = WatchdogConfig::default();
        assert_eq!(config.check_interval, Duration::from_secs(5));
        assert_eq!(config.timeout, Duration::from_secs(2));
        assert_eq!(config.max_consecutive_timeouts, 3);
    }

    #[tokio::test]
    async fn watchdog_resets_counter_on_successful_acquire() {
        let dir = TempDir::new().unwrap();
        let metrics = Arc::new(DaemonMetrics::new());
        let ring = Arc::new(RingWriter::open(dir.path()).unwrap());
        let server = Arc::new(Mutex::new(create_test_server()));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let epoch = Instant::now();

        let state = spawn_watchdog_with_config(
            Arc::clone(&server),
            Arc::clone(&metrics),
            Arc::clone(&ring),
            dir.path().to_path_buf(),
            shutdown_rx,
            epoch,
            &test_config(),
        );

        // Pre-set the counter to simulate prior timeouts.
        state.consecutive_timeouts.store(2, Ordering::Relaxed);

        // Wait for one check cycle (the mutex is free, so it should succeed).
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert_eq!(state.consecutive_timeouts.load(Ordering::Relaxed), 0);

        let _ = shutdown_tx.send(true);
    }

    #[tokio::test]
    async fn watchdog_increments_counter_on_timeout() {
        let dir = TempDir::new().unwrap();
        let metrics = Arc::new(DaemonMetrics::new());
        let ring = Arc::new(RingWriter::open(dir.path()).unwrap());
        let server = Arc::new(Mutex::new(create_test_server()));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let epoch = Instant::now();

        // Hold the mutex so the watchdog times out.
        let guard = server.lock().await;

        let state = spawn_watchdog_with_config(
            Arc::clone(&server),
            Arc::clone(&metrics),
            Arc::clone(&ring),
            dir.path().to_path_buf(),
            shutdown_rx,
            epoch,
            &test_config(),
        );

        // Wait for one check + timeout.
        tokio::time::sleep(Duration::from_millis(250)).await;

        assert!(state.consecutive_timeouts.load(Ordering::Relaxed) >= 1);

        drop(guard);
        let _ = shutdown_tx.send(true);
    }

    #[tokio::test]
    async fn watchdog_writes_ring_event_on_timeout() {
        let dir = TempDir::new().unwrap();
        let metrics = Arc::new(DaemonMetrics::new());
        let ring = Arc::new(RingWriter::open(dir.path()).unwrap());
        let server = Arc::new(Mutex::new(create_test_server()));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let epoch = Instant::now();

        let guard = server.lock().await;

        let _state = spawn_watchdog_with_config(
            Arc::clone(&server),
            Arc::clone(&metrics),
            Arc::clone(&ring),
            dir.path().to_path_buf(),
            shutdown_rx,
            epoch,
            &test_config(),
        );

        tokio::time::sleep(Duration::from_millis(250)).await;

        drop(guard);
        let _ = shutdown_tx.send(true);

        // Verify ring buffer has a WatchdogTimeout event.
        let reader = crate::flight::RingReader::open(&dir.path().join("flight.bin")).unwrap();
        let events = reader.read_all();
        assert!(
            events
                .iter()
                .any(|e| e.span_kind == SpanKind::WatchdogTimeout
                    && e.event_type == EventType::Event),
            "expected WatchdogTimeout event in ring buffer"
        );
    }

    #[tokio::test]
    async fn watchdog_generates_hang_report_after_max_timeouts() {
        let dir = TempDir::new().unwrap();
        let metrics = Arc::new(DaemonMetrics::new());
        metrics.connected_clients.store(3, Ordering::Relaxed);
        metrics.active_panes.store(7, Ordering::Relaxed);
        let ring = Arc::new(RingWriter::open(dir.path()).unwrap());
        let server = Arc::new(Mutex::new(create_test_server()));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let epoch = Instant::now();

        let guard = server.lock().await;

        let state = spawn_watchdog_with_config(
            Arc::clone(&server),
            Arc::clone(&metrics),
            Arc::clone(&ring),
            dir.path().to_path_buf(),
            shutdown_rx,
            epoch,
            &test_config(),
        );

        // Wait for 3 consecutive timeouts: 3 × (100ms interval + 50ms timeout) + margin.
        tokio::time::sleep(Duration::from_millis(600)).await;

        assert!(state.consecutive_timeouts.load(Ordering::Relaxed) >= 3);

        drop(guard);
        let _ = shutdown_tx.send(true);

        // Verify hang report was written.
        let report_path = dir.path().join("hang-report.txt");
        assert!(report_path.exists(), "hang-report.txt should exist");

        let content = std::fs::read_to_string(&report_path).unwrap();
        assert!(content.contains("rttx-server hang report"));
        assert!(content.contains("Consecutive timeouts:"));
        assert!(content.contains("Connected clients: 3"));
        assert!(content.contains("Active panes: 7"));
    }

    #[tokio::test]
    async fn watchdog_stops_on_shutdown() {
        let dir = TempDir::new().unwrap();
        let metrics = Arc::new(DaemonMetrics::new());
        let ring = Arc::new(RingWriter::open(dir.path()).unwrap());
        let server = Arc::new(Mutex::new(create_test_server()));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let epoch = Instant::now();

        let _state = spawn_watchdog_with_config(
            Arc::clone(&server),
            Arc::clone(&metrics),
            Arc::clone(&ring),
            dir.path().to_path_buf(),
            shutdown_rx,
            epoch,
            &test_config(),
        );

        // Signal shutdown immediately.
        let _ = shutdown_tx.send(true);

        // Give the task time to exit.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // The task should have exited without panicking.
    }

    #[tokio::test]
    async fn watchdog_does_not_interfere_with_normal_operation() {
        let dir = TempDir::new().unwrap();
        let metrics = Arc::new(DaemonMetrics::new());
        let ring = Arc::new(RingWriter::open(dir.path()).unwrap());
        let server = Arc::new(Mutex::new(create_test_server()));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let epoch = Instant::now();

        let _state = spawn_watchdog_with_config(
            Arc::clone(&server),
            Arc::clone(&metrics),
            Arc::clone(&ring),
            dir.path().to_path_buf(),
            shutdown_rx,
            epoch,
            &test_config(),
        );

        // Verify we can still acquire the server mutex normally while
        // the watchdog is running.
        for _ in 0..5 {
            let guard = server.lock().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
            drop(guard);
        }

        let _ = shutdown_tx.send(true);

        // No hang report should exist since the mutex was always available.
        let report_path = dir.path().join("hang-report.txt");
        assert!(!report_path.exists());
    }

    fn create_test_server() -> Server {
        use crate::os::OsInterface;
        use std::path::PathBuf;

        #[derive(Debug)]
        struct TestOs;
        impl OsInterface for TestOs {
            fn runtime_dir(&self) -> PathBuf {
                PathBuf::from("/tmp/test-workspace")
            }
            fn cache_dir(&self) -> PathBuf {
                PathBuf::from("/tmp/test-cache")
            }
            fn state_dir(&self) -> PathBuf {
                PathBuf::from("/tmp/test-state/rttx/daemon")
            }
        }
        Server::new(Box::new(TestOs), Arc::new(DaemonMetrics::new()), {
            let dir = tempfile::TempDir::new().unwrap();
            let ring = std::sync::Arc::new(crate::flight::RingWriter::open(dir.path()).unwrap());
            std::mem::forget(dir);
            ring
        })
    }
}
