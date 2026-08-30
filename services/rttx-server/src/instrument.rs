//! Profiling instrumentation for mutex acquisition and channel operations.
//!
//! Provides helpers that wrap lock acquisitions with `tracing` profiling spans
//! and track hold durations via RAII guards. Channel helpers record depth and
//! overflow metrics.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use tokio::sync::{MutexGuard, mpsc};
use tracing::Instrument;

use crate::metrics::DaemonMetrics;

/// Threshold above which a mutex wait is considered contention.
const CONTENTION_THRESHOLD_US: u64 = 1_000; // 1ms

/// Threshold above which a mutex hold is considered too long.
const LONG_HOLD_THRESHOLD_US: u64 = 10_000; // 10ms

/// RAII guard that tracks mutex hold duration and increments
/// `mutex_long_holds` when the hold exceeds the threshold.
pub struct InstrumentedMutexGuard<'a, T> {
    guard: MutexGuard<'a, T>,
    acquired_at: Instant,
    metrics: Arc<DaemonMetrics>,
    target_lock: &'static str,
}

impl<T> Deref for InstrumentedMutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T> DerefMut for InstrumentedMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

impl<T> Drop for InstrumentedMutexGuard<'_, T> {
    fn drop(&mut self) {
        let hold_us = self.acquired_at.elapsed().as_micros() as u64;
        if hold_us > LONG_HOLD_THRESHOLD_US {
            self.metrics.mutex_long_holds.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(
                hold_us,
                target_lock = self.target_lock,
                "mutex held longer than threshold",
            );
        }
    }
}

/// Record a mutex-wait sample, incrementing the contention counter when the
/// wait exceeded [`CONTENTION_THRESHOLD_US`]. Centralises the check shared by
/// [`lock_server`] and [`lock_workspace`] so it can be unit-tested without
/// relying on wall-clock timing.
fn note_contention(metrics: &DaemonMetrics, wait_us: u64) {
    if wait_us > CONTENTION_THRESHOLD_US {
        metrics.mutex_contentions.fetch_add(1, Ordering::Relaxed);
    }
}

/// Acquire the server mutex with profiling instrumentation.
///
/// Records wait time via a `tracing` profiling span and hold time via
/// the returned RAII guard.
pub async fn lock_server<'a>(
    mutex: &'a tokio::sync::Mutex<crate::server::Server>,
    metrics: &Arc<DaemonMetrics>,
) -> InstrumentedMutexGuard<'a, crate::server::Server> {
    let wait_start = Instant::now();
    let span = tracing::info_span!(
        target: "rttx_profile",
        "mutex.acquire",
        span_kind = "mutex_acquire",
        target_lock = "server",
    );
    let guard = mutex.lock().instrument(span).await;
    let wait_us = wait_start.elapsed().as_micros() as u64;
    note_contention(metrics, wait_us);
    InstrumentedMutexGuard {
        guard,
        acquired_at: Instant::now(),
        metrics: Arc::clone(metrics),
        target_lock: "server",
    }
}

/// Acquire a per-workspace mutex with profiling instrumentation.
///
/// Records wait time via a `tracing` profiling span and hold time via
/// the returned RAII guard.
pub async fn lock_workspace<'a>(
    mutex: &'a tokio::sync::Mutex<crate::workspace::Workspace>,
    metrics: &Arc<DaemonMetrics>,
) -> InstrumentedMutexGuard<'a, crate::workspace::Workspace> {
    let wait_start = Instant::now();
    let span = tracing::info_span!(
        target: "rttx_profile",
        "mutex.acquire",
        span_kind = "mutex_acquire",
        target_lock = "workspace",
    );
    let guard = mutex.lock().instrument(span).await;
    let wait_us = wait_start.elapsed().as_micros() as u64;
    note_contention(metrics, wait_us);
    InstrumentedMutexGuard {
        guard,
        acquired_at: Instant::now(),
        metrics: Arc::clone(metrics),
        target_lock: "workspace",
    }
}

/// Send a message via `try_send`, recording channel depth and overflow metrics.
pub fn instrumented_try_send<T>(
    sender: &mpsc::Sender<T>,
    msg: T,
    metrics: &DaemonMetrics,
) -> Result<(), mpsc::error::TrySendError<T>> {
    // Record current channel depth before sending.
    let capacity = sender.capacity();
    let max_capacity = sender.max_capacity();
    let depth = max_capacity.saturating_sub(capacity) as u64;
    metrics.total_channel_depth.store(depth, Ordering::Relaxed);

    let result = sender.try_send(msg);
    if matches!(result, Err(mpsc::error::TrySendError::Full(_))) {
        metrics.channel_overflows.fetch_add(1, Ordering::Relaxed);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::DaemonMetrics;

    #[test]
    fn contention_threshold_is_1ms() {
        assert_eq!(CONTENTION_THRESHOLD_US, 1_000);
    }

    #[test]
    fn long_hold_threshold_is_10ms() {
        assert_eq!(LONG_HOLD_THRESHOLD_US, 10_000);
    }

    #[tokio::test]
    async fn instrumented_try_send_records_depth() {
        let metrics = Arc::new(DaemonMetrics::new());
        let (tx, _rx) = mpsc::channel::<u32>(16);

        let result = instrumented_try_send(&tx, 42, &metrics);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn instrumented_try_send_increments_overflow_on_full() {
        let metrics = Arc::new(DaemonMetrics::new());
        let (tx, _rx) = mpsc::channel::<u32>(1);

        // Fill the channel.
        let _ = tx.try_send(1);
        // Next send should overflow.
        let result = instrumented_try_send(&tx, 2, &metrics);
        assert!(result.is_err());
        assert_eq!(metrics.channel_overflows.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn instrumented_try_send_does_not_increment_on_success() {
        let metrics = Arc::new(DaemonMetrics::new());
        let (tx, _rx) = mpsc::channel::<u32>(16);

        let _ = instrumented_try_send(&tx, 1, &metrics);
        assert_eq!(metrics.channel_overflows.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn instrumented_try_send_records_channel_depth() {
        let metrics = Arc::new(DaemonMetrics::new());
        let (tx, _rx) = mpsc::channel::<u32>(16);

        // Pre-fill 3 messages.
        let _ = tx.try_send(1);
        let _ = tx.try_send(2);
        let _ = tx.try_send(3);

        let _ = instrumented_try_send(&tx, 4, &metrics);
        // Depth should be 3 (pre-send depth).
        assert_eq!(metrics.total_channel_depth.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn lock_server_returns_working_guard() {
        let metrics = Arc::new(DaemonMetrics::new());
        let os = crate::os::unix::UnixOs;
        let dir = tempfile::TempDir::new().unwrap();
        let ring = Arc::new(crate::flight::RingWriter::open(dir.path()).unwrap());
        let server = tokio::sync::Mutex::new(crate::server::Server::new(
            Box::new(os),
            Arc::clone(&metrics),
            ring,
        ));

        let guard = lock_server(&server, &metrics).await;
        assert!(!guard.server_id.is_nil());
        drop(guard);
    }

    #[tokio::test]
    async fn lock_workspace_returns_working_guard() {
        let metrics = Arc::new(DaemonMetrics::new());
        let rt = crate::workspace::Workspace::new("test".to_string());
        let mutex = tokio::sync::Mutex::new(rt);

        let guard = lock_workspace(&mutex, &metrics).await;
        assert_eq!(guard.name, "test");
        drop(guard);
    }

    #[tokio::test]
    async fn long_hold_increments_metric_on_drop() {
        let metrics = Arc::new(DaemonMetrics::new());
        let rt = crate::workspace::Workspace::new("test".to_string());
        let mutex = tokio::sync::Mutex::new(rt);

        {
            let _guard = lock_workspace(&mutex, &metrics).await;
            // Simulate a long hold.
            tokio::time::sleep(std::time::Duration::from_millis(12)).await;
        }

        assert_eq!(metrics.mutex_long_holds.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn short_hold_does_not_increment_long_hold_metric() {
        let metrics = Arc::new(DaemonMetrics::new());
        let rt = crate::workspace::Workspace::new("test".to_string());
        let mutex = tokio::sync::Mutex::new(rt);

        {
            let _guard = lock_workspace(&mutex, &metrics).await;
        }

        assert_eq!(metrics.mutex_long_holds.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn note_contention_counts_only_above_threshold() {
        let metrics = DaemonMetrics::new();

        // At or below the threshold: not counted as contention.
        note_contention(&metrics, 0);
        note_contention(&metrics, CONTENTION_THRESHOLD_US - 1);
        note_contention(&metrics, CONTENTION_THRESHOLD_US);
        assert_eq!(metrics.mutex_contentions.load(Ordering::Relaxed), 0);

        // Strictly above the threshold: one contention recorded per call.
        note_contention(&metrics, CONTENTION_THRESHOLD_US + 1);
        note_contention(&metrics, 10 * CONTENTION_THRESHOLD_US);
        assert_eq!(metrics.mutex_contentions.load(Ordering::Relaxed), 2);
    }
}
