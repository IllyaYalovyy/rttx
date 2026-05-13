//! Crash report generation for panic hook.
//!
//! Writes a human-readable crash report containing panic info, metrics
//! snapshot, and recent ring buffer events to `$CACHE_DIR/crash-report.txt`.

use std::fmt::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::flight::{EventType, FlightEvent, RingReader, RingWriter, SpanKind};
use crate::metrics::DaemonMetrics;

/// Maximum number of recent ring buffer entries to include in the crash report.
const MAX_RECENT_EVENTS: usize = 100;

/// Write a PANIC event to the ring buffer and flush to disk.
pub fn record_panic_event(ring: &RingWriter, epoch: Instant) {
    let timestamp_ns = epoch.elapsed().as_nanos() as u64;
    ring.record(&FlightEvent {
        timestamp_ns,
        span_id: 0,
        event_type: EventType::Panic,
        span_kind: SpanKind::Shutdown,
        context: [0; 16],
        value: 0,
    });
    let _ = ring.flush();
}

/// Generate a crash report file at the given path.
///
/// Contains panic info, metrics snapshot, and recent ring buffer events.
pub fn write_crash_report(
    report_path: &Path,
    panic_message: &str,
    panic_location: &str,
    metrics: &Arc<DaemonMetrics>,
    ring: &Arc<RingWriter>,
    start_time: Instant,
) {
    let mut report = String::with_capacity(8192);

    let uptime = start_time.elapsed();
    let timestamp = chrono_free_timestamp();

    // Header
    let _ = writeln!(report, "rttx-server crash report");
    let _ = writeln!(report, "========================");
    let _ = writeln!(report, "Timestamp: {timestamp}");
    let _ = writeln!(report, "PID: {}", std::process::id());
    let _ = writeln!(report, "Uptime: {:.1}s", uptime.as_secs_f64());
    let _ = writeln!(report);

    // Panic info
    let _ = writeln!(report, "── Panic ───────────────────────────────────────────────────");
    let _ = writeln!(report, "Message: {panic_message}");
    let _ = writeln!(report, "Location: {panic_location}");
    let _ = writeln!(report);

    // Metrics snapshot
    let _ = writeln!(report, "── Metrics ─────────────────────────────────────────────────");
    write_metrics_snapshot(&mut report, metrics);
    let _ = writeln!(report);

    // Recent ring buffer events
    let _ =
        writeln!(report, "── Recent events (last {MAX_RECENT_EVENTS}) ─────────────────────────");
    write_recent_events(&mut report, ring);

    let _ = std::fs::write(report_path, &report);
}

fn write_metrics_snapshot(out: &mut String, metrics: &DaemonMetrics) {
    let _ = writeln!(out, "Gauges:");
    let _ =
        writeln!(out, "  connected_clients: {}", metrics.connected_clients.load(Ordering::Relaxed));
    let _ = writeln!(out, "  active_panes: {}", metrics.active_panes.load(Ordering::Relaxed));
    let _ = writeln!(
        out,
        "  total_channel_depth: {}",
        metrics.total_channel_depth.load(Ordering::Relaxed)
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "Counters:");
    let _ = writeln!(
        out,
        "  messages_dispatched: {}",
        metrics.messages_dispatched.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "  bytes_read_from_pty: {}",
        metrics.bytes_read_from_pty.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "  bytes_written_to_clients: {}",
        metrics.bytes_written_to_clients.load(Ordering::Relaxed)
    );
    let _ =
        writeln!(out, "  channel_overflows: {}", metrics.channel_overflows.load(Ordering::Relaxed));
    let _ =
        writeln!(out, "  mutex_contentions: {}", metrics.mutex_contentions.load(Ordering::Relaxed));
    let _ =
        writeln!(out, "  mutex_long_holds: {}", metrics.mutex_long_holds.load(Ordering::Relaxed));
    let _ = writeln!(
        out,
        "  serialization_ticks: {}",
        metrics.serialization_ticks.load(Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "  client_disconnects: {}",
        metrics.client_disconnects.load(Ordering::Relaxed)
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "Histograms:");
    write_histogram(out, "  mutex_wait_us", &metrics.mutex_wait_us);
    write_histogram(out, "  dispatch_latency_us", &metrics.dispatch_latency_us);
    write_histogram(out, "  pty_read_latency_us", &metrics.pty_read_latency_us);
    write_histogram(out, "  client_write_latency_us", &metrics.client_write_latency_us);
}

fn write_histogram(out: &mut String, label: &str, h: &crate::metrics::LatencyHistogram) {
    let snap = h.snapshot();
    let total: u64 = snap.iter().sum();
    let _ = writeln!(
        out,
        "{label}: [<10µs:{}, <100µs:{}, <1ms:{}, <10ms:{}, <100ms:{}, ≥100ms:{}] total={total}",
        snap[0], snap[1], snap[2], snap[3], snap[4], snap[5]
    );
}

fn write_recent_events(out: &mut String, ring: &RingWriter) {
    let path = ring.path();
    let Ok(reader) = RingReader::open(path) else {
        let _ = writeln!(out, "(unable to read ring buffer)");
        return;
    };

    let all_events = reader.read_all();
    let start = all_events.len().saturating_sub(MAX_RECENT_EVENTS);
    let recent = &all_events[start..];

    if recent.is_empty() {
        let _ = writeln!(out, "(no events recorded)");
        return;
    }

    for event in recent {
        let ts = format_ns_timestamp(event.timestamp_ns);
        let kind = span_kind_label(event.span_kind);
        let etype = match event.event_type {
            EventType::Enter => "ENTER",
            EventType::Exit => "EXIT ",
            EventType::Event => "EVENT",
            EventType::Panic => "PANIC",
        };
        let value_str = if event.event_type == EventType::Exit && event.value > 0 {
            let dur_us = event.value / 1_000;
            if dur_us >= 1_000 {
                format!(" dur={:.1}ms", dur_us as f64 / 1_000.0)
            } else {
                format!(" dur={dur_us}µs")
            }
        } else {
            String::new()
        };
        let _ = writeln!(out, "[{ts}] span={:<6} {etype} {kind:<24}{value_str}", event.span_id);
    }
}

const fn span_kind_label(kind: SpanKind) -> &'static str {
    match kind {
        SpanKind::MutexAcquire => "mutex.acquire",
        SpanKind::PtyRead => "pty.read",
        SpanKind::VteParse => "vte.parse",
        SpanKind::ClientDispatch => "client.dispatch",
        SpanKind::ClientWrite => "client.write",
        SpanKind::ChannelSend => "channel.send",
        SpanKind::SerializationTick => "serialization.tick",
        SpanKind::IoFlush => "io.flush",
        SpanKind::ClientSession => "client.session",
        SpanKind::Shutdown => "shutdown",
    }
}

fn format_ns_timestamp(ns: u64) -> String {
    let total_secs = ns / 1_000_000_000;
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

/// Produce a timestamp without pulling in the `chrono` crate.
fn chrono_free_timestamp() -> String {
    use std::time::SystemTime;
    let dur = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let secs = dur.as_secs();
    // Simple UTC timestamp: seconds since epoch
    format!("{secs} (unix epoch seconds)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (Arc<DaemonMetrics>, Arc<RingWriter>, TempDir) {
        let dir = TempDir::new().unwrap();
        let metrics = Arc::new(DaemonMetrics::new());
        let ring = Arc::new(RingWriter::open(dir.path()).unwrap());
        (metrics, ring, dir)
    }

    #[test]
    fn record_panic_event_writes_to_ring_buffer() {
        let (_metrics, ring, dir) = setup();
        let epoch = Instant::now();

        record_panic_event(&ring, epoch);

        let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
        let events = reader.read_all();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::Panic);
        assert_eq!(events[0].span_kind, SpanKind::Shutdown);
    }

    #[test]
    fn write_crash_report_creates_file_with_expected_sections() {
        let (metrics, ring, dir) = setup();
        let start_time = Instant::now();
        let report_path = dir.path().join("crash-report.txt");

        // Record some events so the report has data
        ring.record(&FlightEvent {
            timestamp_ns: 1_000_000_000,
            span_id: 1,
            event_type: EventType::Enter,
            span_kind: SpanKind::PtyRead,
            context: [0; 16],
            value: 0,
        });
        ring.record(&FlightEvent {
            timestamp_ns: 2_000_000_000,
            span_id: 1,
            event_type: EventType::Exit,
            span_kind: SpanKind::PtyRead,
            context: [0; 16],
            value: 500_000,
        });

        metrics.messages_dispatched.fetch_add(42, Ordering::Relaxed);
        metrics.connected_clients.fetch_add(2, Ordering::Relaxed);

        write_crash_report(
            &report_path,
            "test panic message",
            "src/server.rs:123:5",
            &metrics,
            &ring,
            start_time,
        );

        let content = std::fs::read_to_string(&report_path).unwrap();
        assert!(content.contains("rttx-server crash report"));
        assert!(content.contains("test panic message"));
        assert!(content.contains("src/server.rs:123:5"));
        assert!(content.contains("messages_dispatched: 42"));
        assert!(content.contains("connected_clients: 2"));
        assert!(content.contains("pty.read"));
        assert!(content.contains("ENTER"));
        assert!(content.contains("EXIT"));
    }

    #[test]
    fn write_crash_report_with_empty_ring_buffer() {
        let (metrics, ring, dir) = setup();
        let start_time = Instant::now();
        let report_path = dir.path().join("crash-report.txt");

        write_crash_report(
            &report_path,
            "empty buffer panic",
            "unknown:0:0",
            &metrics,
            &ring,
            start_time,
        );

        let content = std::fs::read_to_string(&report_path).unwrap();
        assert!(content.contains("empty buffer panic"));
        assert!(content.contains("(no events recorded)"));
    }

    #[test]
    fn write_crash_report_limits_events_to_max() {
        let (metrics, ring, dir) = setup();
        let start_time = Instant::now();
        let report_path = dir.path().join("crash-report.txt");

        // Write more than MAX_RECENT_EVENTS
        for i in 0..200 {
            ring.record(&FlightEvent {
                timestamp_ns: i * 1_000_000,
                span_id: i as u32,
                event_type: EventType::Enter,
                span_kind: SpanKind::PtyRead,
                context: [0; 16],
                value: 0,
            });
        }

        write_crash_report(&report_path, "overflow panic", "test:1:1", &metrics, &ring, start_time);

        let content = std::fs::read_to_string(&report_path).unwrap();
        let event_count =
            content.lines().filter(|l| l.starts_with('[') && l.contains("ENTER")).count();
        assert_eq!(event_count, MAX_RECENT_EVENTS);
    }

    #[test]
    fn crash_report_includes_pid_and_uptime() {
        let (metrics, ring, dir) = setup();
        let start_time = Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let report_path = dir.path().join("crash-report.txt");

        write_crash_report(&report_path, "uptime test", "test:1:1", &metrics, &ring, start_time);

        let content = std::fs::read_to_string(&report_path).unwrap();
        assert!(content.contains(&format!("PID: {}", std::process::id())));
        assert!(content.contains("Uptime:"));
    }

    #[test]
    fn crash_report_includes_histogram_data() {
        let (metrics, ring, dir) = setup();
        let start_time = Instant::now();
        let report_path = dir.path().join("crash-report.txt");

        metrics.mutex_wait_us.record(50);
        metrics.mutex_wait_us.record(500);

        write_crash_report(&report_path, "histogram test", "test:1:1", &metrics, &ring, start_time);

        let content = std::fs::read_to_string(&report_path).unwrap();
        assert!(content.contains("mutex_wait_us:"));
        assert!(content.contains("total=2"));
    }
}
