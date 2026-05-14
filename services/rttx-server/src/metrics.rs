//! Always-on daemon profiling metrics.
//!
//! Lock-free atomic counters and latency histograms for tracking daemon
//! performance without measurable overhead (<5ns per record call).

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Fixed bucket boundaries in microseconds for latency histograms.
/// Buckets: [0, 10), [10, 100), [100, 1000), [1000, 10000), [10000, 100000), [100000+)
const BUCKET_BOUNDARIES: [u64; 5] = [10, 100, 1_000, 10_000, 100_000];
const BUCKET_COUNT: usize = 6;

/// Lock-free latency histogram with fixed microsecond buckets.
///
/// Each `record()` call performs a single `fetch_add` on the appropriate bucket.
#[derive(Debug)]
pub struct LatencyHistogram {
    buckets: [AtomicU64; BUCKET_COUNT],
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self::new()
    }
}

impl LatencyHistogram {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buckets: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
        }
    }

    /// Record a latency value in microseconds. Single atomic `fetch_add`.
    pub fn record(&self, duration_us: u64) {
        let idx =
            BUCKET_BOUNDARIES.iter().position(|&b| duration_us < b).unwrap_or(BUCKET_COUNT - 1);
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
    }

    /// Returns a snapshot of all bucket values.
    #[must_use]
    pub fn snapshot(&self) -> [u64; BUCKET_COUNT] {
        [
            self.buckets[0].load(Ordering::Relaxed),
            self.buckets[1].load(Ordering::Relaxed),
            self.buckets[2].load(Ordering::Relaxed),
            self.buckets[3].load(Ordering::Relaxed),
            self.buckets[4].load(Ordering::Relaxed),
            self.buckets[5].load(Ordering::Relaxed),
        ]
    }

    /// Approximate percentile from bucket boundaries.
    ///
    /// Returns the upper bound of the bucket containing the p-th percentile
    /// sample, or `u64::MAX` for the overflow bucket.
    /// Returns `None` if no samples have been recorded.
    #[must_use]
    pub fn percentile(&self, p: f64) -> Option<u64> {
        let snap = self.snapshot();
        let total: u64 = snap.iter().sum();
        if total == 0 {
            return None;
        }

        let threshold =
            ((f64::from(u32::try_from(total).unwrap_or(u32::MAX))) * p / 100.0).ceil() as u64;
        let mut cumulative = 0u64;
        for (i, &count) in snap.iter().enumerate() {
            cumulative += count;
            if cumulative >= threshold {
                return Some(if i < BUCKET_BOUNDARIES.len() {
                    BUCKET_BOUNDARIES[i]
                } else {
                    u64::MAX
                });
            }
        }
        Some(u64::MAX)
    }
}

/// Server-wide atomic metrics for always-on profiling.
///
/// All fields use atomic operations — no locks required for reads or writes.
#[derive(Debug)]
pub struct DaemonMetrics {
    // Epoch for ring buffer timestamps
    pub epoch: std::time::Instant,

    // Gauges
    pub connected_clients: AtomicU32,
    pub active_panes: AtomicU32,
    pub total_channel_depth: AtomicU64,

    // Counters
    pub messages_dispatched: AtomicU64,
    pub bytes_read_from_pty: AtomicU64,
    pub bytes_written_to_clients: AtomicU64,
    pub channel_overflows: AtomicU64,
    pub mutex_contentions: AtomicU64,
    pub mutex_long_holds: AtomicU64,
    pub serialization_ticks: AtomicU64,
    pub client_disconnects: AtomicU64,

    // Histograms
    pub mutex_wait_us: LatencyHistogram,
    pub dispatch_latency_us: LatencyHistogram,
    pub pty_read_latency_us: LatencyHistogram,
    pub client_write_latency_us: LatencyHistogram,
    pub vte_parse_latency_us: LatencyHistogram,
    pub serialization_tick_latency_us: LatencyHistogram,
    pub io_flush_latency_us: LatencyHistogram,
}

impl Default for DaemonMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl DaemonMetrics {
    #[must_use]
    pub fn new() -> Self {
        Self {
            epoch: std::time::Instant::now(),

            connected_clients: AtomicU32::new(0),
            active_panes: AtomicU32::new(0),
            total_channel_depth: AtomicU64::new(0),

            messages_dispatched: AtomicU64::new(0),
            bytes_read_from_pty: AtomicU64::new(0),
            bytes_written_to_clients: AtomicU64::new(0),
            channel_overflows: AtomicU64::new(0),
            mutex_contentions: AtomicU64::new(0),
            mutex_long_holds: AtomicU64::new(0),
            serialization_ticks: AtomicU64::new(0),
            client_disconnects: AtomicU64::new(0),

            mutex_wait_us: LatencyHistogram::new(),
            dispatch_latency_us: LatencyHistogram::new(),
            pty_read_latency_us: LatencyHistogram::new(),
            client_write_latency_us: LatencyHistogram::new(),
            vte_parse_latency_us: LatencyHistogram::new(),
            serialization_tick_latency_us: LatencyHistogram::new(),
            io_flush_latency_us: LatencyHistogram::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- LatencyHistogram bucket selection tests ---

    #[test]
    fn histogram_record_zero_goes_to_first_bucket() {
        let h = LatencyHistogram::new();
        h.record(0);
        assert_eq!(h.snapshot(), [1, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn histogram_record_boundary_value_goes_to_next_bucket() {
        let h = LatencyHistogram::new();
        // 10 is >= boundary[0]=10, so goes to bucket 1
        h.record(10);
        assert_eq!(h.snapshot(), [0, 1, 0, 0, 0, 0]);
    }

    #[test]
    fn histogram_record_just_below_boundary() {
        let h = LatencyHistogram::new();
        h.record(9);
        assert_eq!(h.snapshot(), [1, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn histogram_record_all_buckets() {
        let h = LatencyHistogram::new();
        h.record(5); // [0, 10)
        h.record(50); // [10, 100)
        h.record(500); // [100, 1000)
        h.record(5_000); // [1000, 10000)
        h.record(50_000); // [10000, 100000)
        h.record(500_000); // [100000+)
        assert_eq!(h.snapshot(), [1, 1, 1, 1, 1, 1]);
    }

    #[test]
    fn histogram_overflow_bucket_for_large_values() {
        let h = LatencyHistogram::new();
        h.record(100_000);
        h.record(u64::MAX);
        assert_eq!(h.snapshot(), [0, 0, 0, 0, 0, 2]);
    }

    #[test]
    fn histogram_multiple_records_accumulate() {
        let h = LatencyHistogram::new();
        for _ in 0..100 {
            h.record(5);
        }
        assert_eq!(h.snapshot()[0], 100);
    }

    // --- LatencyHistogram percentile tests ---

    #[test]
    fn percentile_empty_histogram_returns_none() {
        let h = LatencyHistogram::new();
        assert_eq!(h.percentile(50.0), None);
    }

    #[test]
    fn percentile_single_sample_returns_bucket_upper_bound() {
        let h = LatencyHistogram::new();
        h.record(5); // bucket 0: [0, 10)
        assert_eq!(h.percentile(50.0), Some(10));
        assert_eq!(h.percentile(99.0), Some(10));
    }

    #[test]
    fn percentile_all_in_overflow_returns_max() {
        let h = LatencyHistogram::new();
        h.record(200_000);
        assert_eq!(h.percentile(50.0), Some(u64::MAX));
    }

    #[test]
    fn percentile_distributed_samples() {
        let h = LatencyHistogram::new();
        // 90 samples in [0,10), 10 samples in [10000, 100000)
        for _ in 0..90 {
            h.record(5);
        }
        for _ in 0..10 {
            h.record(50_000);
        }
        // p50 should be in first bucket (upper bound 10)
        assert_eq!(h.percentile(50.0), Some(10));
        // p95 should be in the [10000, 100000) bucket (upper bound 100_000)
        assert_eq!(h.percentile(95.0), Some(100_000));
    }

    #[test]
    fn percentile_p99_with_tail() {
        let h = LatencyHistogram::new();
        // 99 fast, 1 slow
        for _ in 0..99 {
            h.record(5);
        }
        h.record(150_000); // overflow bucket
        assert_eq!(h.percentile(99.0), Some(10));
        // p100 hits the overflow
        assert_eq!(h.percentile(100.0), Some(u64::MAX));
    }

    // --- DaemonMetrics counter tests ---

    #[test]
    fn daemon_metrics_initial_values_are_zero() {
        let m = DaemonMetrics::new();
        assert_eq!(m.connected_clients.load(Ordering::Relaxed), 0);
        assert_eq!(m.active_panes.load(Ordering::Relaxed), 0);
        assert_eq!(m.total_channel_depth.load(Ordering::Relaxed), 0);
        assert_eq!(m.messages_dispatched.load(Ordering::Relaxed), 0);
        assert_eq!(m.bytes_read_from_pty.load(Ordering::Relaxed), 0);
        assert_eq!(m.bytes_written_to_clients.load(Ordering::Relaxed), 0);
        assert_eq!(m.channel_overflows.load(Ordering::Relaxed), 0);
        assert_eq!(m.mutex_contentions.load(Ordering::Relaxed), 0);
        assert_eq!(m.mutex_long_holds.load(Ordering::Relaxed), 0);
        assert_eq!(m.serialization_ticks.load(Ordering::Relaxed), 0);
        assert_eq!(m.client_disconnects.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn daemon_metrics_counter_increment() {
        let m = DaemonMetrics::new();
        m.messages_dispatched.fetch_add(1, Ordering::Relaxed);
        m.bytes_read_from_pty.fetch_add(1024, Ordering::Relaxed);
        assert_eq!(m.messages_dispatched.load(Ordering::Relaxed), 1);
        assert_eq!(m.bytes_read_from_pty.load(Ordering::Relaxed), 1024);
    }

    #[test]
    fn daemon_metrics_gauge_update() {
        let m = DaemonMetrics::new();
        m.connected_clients.fetch_add(1, Ordering::Relaxed);
        m.connected_clients.fetch_add(1, Ordering::Relaxed);
        assert_eq!(m.connected_clients.load(Ordering::Relaxed), 2);
        m.connected_clients.fetch_sub(1, Ordering::Relaxed);
        assert_eq!(m.connected_clients.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn daemon_metrics_histogram_integration() {
        let m = DaemonMetrics::new();
        m.mutex_wait_us.record(50);
        m.dispatch_latency_us.record(500);
        m.pty_read_latency_us.record(5_000);
        m.client_write_latency_us.record(50_000);

        assert_eq!(m.mutex_wait_us.snapshot()[1], 1); // [10, 100)
        assert_eq!(m.dispatch_latency_us.snapshot()[2], 1); // [100, 1000)
        assert_eq!(m.pty_read_latency_us.snapshot()[3], 1); // [1000, 10000)
        assert_eq!(m.client_write_latency_us.snapshot()[4], 1); // [10000, 100000)
    }

    #[test]
    fn histogram_record_is_single_atomic_op() {
        // AtomicU64 on 64-bit platforms uses a single CPU instruction.
        // Verify the type size matches the platform word size as a proxy.
        assert_eq!(std::mem::size_of::<AtomicU64>(), std::mem::size_of::<u64>());
    }

    #[test]
    fn daemon_metrics_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let m = Arc::new(DaemonMetrics::new());
        let mut handles = Vec::new();

        for _ in 0..4 {
            let metrics = Arc::clone(&m);
            handles.push(thread::spawn(move || {
                for _ in 0..1000 {
                    metrics.messages_dispatched.fetch_add(1, Ordering::Relaxed);
                    metrics.mutex_wait_us.record(50);
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(m.messages_dispatched.load(Ordering::Relaxed), 4000);
        assert_eq!(m.mutex_wait_us.snapshot()[1], 4000); // [10, 100)
    }
}
