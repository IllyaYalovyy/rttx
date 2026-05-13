//! Profile report generation from flight recorder data.
//!
//! Reads the ring buffer file directly (no socket communication needed)
//! and computes latency percentiles, contention stats, and recent slow
//! operations from the recorded events.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::time::Duration;

use crate::flight::{EventType, FlightEvent, RingReader, SpanKind};

/// Threshold in nanoseconds above which an operation is considered "slow".
const SLOW_THRESHOLD_NS: u64 = 5_000_000; // 5ms

/// Maximum number of recent slow operations to display.
const MAX_SLOW_OPS: usize = 10;

/// Computed latency percentiles for a span kind.
#[derive(Debug, Clone, Default)]
pub struct LatencyStats {
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub count: u64,
}

/// A single slow operation extracted from the ring buffer.
#[derive(Debug, Clone)]
pub struct SlowOp {
    pub timestamp_ns: u64,
    pub span_kind: SpanKind,
    pub duration_ns: u64,
    pub context: [u8; 16],
}

/// Contention statistics derived from ring buffer events.
#[derive(Debug, Clone, Default)]
pub struct ContentionStats {
    pub mutex_contentions: u64,
    pub long_holds: u64,
    pub channel_overflows: u64,
}

/// Complete profiling report computed from flight recorder data.
#[derive(Debug, Clone)]
pub struct ProfileReport {
    pub pid: Option<u32>,
    pub uptime: Option<Duration>,
    pub total_events: u64,
    pub mutex_latency: LatencyStats,
    pub dispatch_latency: LatencyStats,
    pub pty_read_latency: LatencyStats,
    pub client_write_latency: LatencyStats,
    pub contention: ContentionStats,
    pub slow_ops: Vec<SlowOp>,
}

/// Generate a profile report from a flight recorder file.
pub fn generate_report(flight_path: &Path, pid: Option<u32>) -> std::io::Result<ProfileReport> {
    let reader = RingReader::open(flight_path)?;
    let events = reader.read_all();
    Ok(build_report(&events, pid))
}

/// Build a report from a slice of events (testable without file I/O).
#[must_use]
pub fn build_report(events: &[FlightEvent], pid: Option<u32>) -> ProfileReport {
    let total_events = events.len() as u64;

    // Collect exit-event durations by span kind.
    let mut durations: HashMap<SpanKind, Vec<u64>> = HashMap::new();
    let mut slow_ops = Vec::new();
    let mut contention = ContentionStats::default();

    // Track uptime from first to last event timestamp.
    let uptime = if events.len() >= 2 {
        let first_ts = events.iter().map(|e| e.timestamp_ns).min().unwrap_or(0);
        let last_ts = events.iter().map(|e| e.timestamp_ns).max().unwrap_or(0);
        Some(Duration::from_nanos(last_ts.saturating_sub(first_ts)))
    } else {
        None
    };

    for event in events {
        if event.event_type != EventType::Exit {
            continue;
        }

        let duration_ns = event.value;
        durations.entry(event.span_kind).or_default().push(duration_ns);

        // Count contention events.
        if event.span_kind == SpanKind::MutexAcquire && duration_ns > 1_000_000 {
            contention.mutex_contentions += 1;
        }
        if event.span_kind == SpanKind::MutexAcquire && duration_ns > 10_000_000 {
            contention.long_holds += 1;
        }

        // Collect slow operations.
        if duration_ns > SLOW_THRESHOLD_NS {
            slow_ops.push(SlowOp {
                timestamp_ns: event.timestamp_ns,
                span_kind: event.span_kind,
                duration_ns,
                context: event.context,
            });
        }
    }

    // Count channel overflow events (ChannelSend events are recorded as Event type).
    for event in events {
        if event.event_type == EventType::Event && event.span_kind == SpanKind::ChannelSend {
            contention.channel_overflows += 1;
        }
    }

    // Sort slow ops by timestamp descending, keep most recent.
    slow_ops.sort_by_key(|op| std::cmp::Reverse(op.timestamp_ns));
    slow_ops.truncate(MAX_SLOW_OPS);

    let mutex_latency = compute_latency_stats(durations.get(&SpanKind::MutexAcquire));
    let dispatch_latency = compute_latency_stats(durations.get(&SpanKind::ClientDispatch));
    let pty_read_latency = compute_latency_stats(durations.get(&SpanKind::PtyRead));
    let client_write_latency = compute_latency_stats(durations.get(&SpanKind::ClientWrite));

    ProfileReport {
        pid,
        uptime,
        total_events,
        mutex_latency,
        dispatch_latency,
        pty_read_latency,
        client_write_latency,
        contention,
        slow_ops,
    }
}

fn compute_latency_stats(durations: Option<&Vec<u64>>) -> LatencyStats {
    let Some(durs) = durations else {
        return LatencyStats::default();
    };
    if durs.is_empty() {
        return LatencyStats::default();
    }

    let mut sorted: Vec<u64> = durs.clone();
    sorted.sort_unstable();
    let count = sorted.len() as u64;

    LatencyStats {
        p50_us: sorted[percentile_index(sorted.len(), 50)] / 1_000,
        p95_us: sorted[percentile_index(sorted.len(), 95)] / 1_000,
        p99_us: sorted[percentile_index(sorted.len(), 99)] / 1_000,
        count,
    }
}

fn percentile_index(len: usize, p: usize) -> usize {
    ((len as f64) * (p as f64) / 100.0).ceil() as usize - 1
}

/// Format a duration in human-readable form.
fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 3600 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        format!("{h}h {m:02}m")
    } else if secs >= 60 {
        let m = secs / 60;
        let s = secs % 60;
        format!("{m}m {s:02}s")
    } else {
        format!("{secs}s")
    }
}

/// Format microseconds in human-readable form.
fn format_us(us: u64) -> String {
    if us >= 1_000_000 {
        format!("{:.1} s", us as f64 / 1_000_000.0)
    } else if us >= 1_000 {
        format!("{:.1} ms", us as f64 / 1_000.0)
    } else {
        format!("{us} µs")
    }
}

impl fmt::Display for ProfileReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Header
        write!(f, "rttx-server profiling report")?;
        if let Some(uptime) = self.uptime {
            write!(f, " (uptime: {}", format_duration(uptime))?;
            if let Some(pid) = self.pid {
                write!(f, ", pid: {pid}")?;
            }
            write!(f, ")")?;
        } else if let Some(pid) = self.pid {
            write!(f, " (pid: {pid})")?;
        }
        writeln!(f)?;

        // Latency section
        writeln!(f)?;
        writeln!(f, "── Latency (p50 / p95 / p99) ──────────────────────────────")?;
        write_latency_line(f, "Mutex wait", &self.mutex_latency)?;
        write_latency_line(f, "Message dispatch", &self.dispatch_latency)?;
        write_latency_line(f, "PTY read batch", &self.pty_read_latency)?;
        write_latency_line(f, "Client write", &self.client_write_latency)?;

        // Contention section
        writeln!(f)?;
        writeln!(f, "── Contention ──────────────────────────────────────────────")?;
        writeln!(f, "Mutex contentions (>1ms): {} total", self.contention.mutex_contentions)?;
        writeln!(f, "Long holds (>10ms):       {} total", self.contention.long_holds)?;
        writeln!(f, "Channel overflows:        {} total", self.contention.channel_overflows)?;

        // Slow operations
        if !self.slow_ops.is_empty() {
            writeln!(f)?;
            writeln!(f, "── Recent slow operations (>5ms) ───────────────────────────")?;
            for op in &self.slow_ops {
                let dur_ms = op.duration_ns as f64 / 1_000_000.0;
                let kind_str = span_kind_display(op.span_kind);
                let ctx = format_context(&op.context);
                write!(
                    f,
                    "[{:>10}] {kind_str:<30} dur={dur_ms:.1}ms",
                    format_ns_timestamp(op.timestamp_ns)
                )?;
                if !ctx.is_empty() {
                    write!(f, "  ({ctx})")?;
                }
                writeln!(f)?;
            }
        }

        Ok(())
    }
}

fn write_latency_line(
    f: &mut fmt::Formatter<'_>,
    label: &str,
    stats: &LatencyStats,
) -> fmt::Result {
    if stats.count == 0 {
        writeln!(f, "{label:<18} (no data)")
    } else {
        writeln!(
            f,
            "{label:<18}{:>8} / {:>8} / {:>8}  (n={})",
            format_us(stats.p50_us),
            format_us(stats.p95_us),
            format_us(stats.p99_us),
            stats.count,
        )
    }
}

const fn span_kind_display(kind: SpanKind) -> &'static str {
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

fn format_context(ctx: &[u8; 16]) -> String {
    if ctx.iter().all(|&b| b == 0) {
        return String::new();
    }
    // Show first 8 bytes as hex (pane/client ID prefix).
    format!("id={:02x}{:02x}{:02x}{:02x}", ctx[0], ctx[1], ctx[2], ctx[3])
}

fn format_ns_timestamp(ns: u64) -> String {
    let total_secs = ns / 1_000_000_000;
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

/// Format events as a chronological dump.
#[must_use]
pub fn format_dump(events: &[FlightEvent]) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "Flight recorder dump ({} events)\n", events.len());
    for event in events {
        let ts = format_ns_timestamp(event.timestamp_ns);
        let kind = span_kind_display(event.span_kind);
        let etype = match event.event_type {
            EventType::Enter => "ENTER",
            EventType::Exit => "EXIT ",
            EventType::Event => "EVENT",
            EventType::Panic => "PANIC",
        };
        let value_str = if event.event_type == EventType::Exit {
            let dur_us = event.value / 1_000;
            format!("dur={}", format_us(dur_us))
        } else if event.value > 0 {
            format!("val={}", event.value)
        } else {
            String::new()
        };
        let _ = writeln!(out, "[{ts}] span={:<6} {etype} {kind:<24} {value_str}", event.span_id);
    }
    out
}

/// JSON-serializable profile report.
#[derive(Debug, serde::Serialize)]
pub struct JsonReport {
    pub pid: Option<u32>,
    pub uptime_secs: Option<f64>,
    pub total_events: u64,
    pub latency: JsonLatency,
    pub contention: JsonContention,
    pub slow_ops: Vec<JsonSlowOp>,
}

#[derive(Debug, serde::Serialize)]
pub struct JsonLatency {
    pub mutex_wait: JsonLatencyStats,
    pub dispatch: JsonLatencyStats,
    pub pty_read: JsonLatencyStats,
    pub client_write: JsonLatencyStats,
}

#[derive(Debug, serde::Serialize)]
pub struct JsonLatencyStats {
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub count: u64,
}

#[derive(Debug, serde::Serialize)]
pub struct JsonContention {
    pub mutex_contentions: u64,
    pub long_holds: u64,
    pub channel_overflows: u64,
}

#[derive(Debug, serde::Serialize)]
pub struct JsonSlowOp {
    pub timestamp_ns: u64,
    pub span_kind: String,
    pub duration_ns: u64,
}

impl From<&ProfileReport> for JsonReport {
    fn from(r: &ProfileReport) -> Self {
        Self {
            pid: r.pid,
            uptime_secs: r.uptime.map(|d| d.as_secs_f64()),
            total_events: r.total_events,
            latency: JsonLatency {
                mutex_wait: (&r.mutex_latency).into(),
                dispatch: (&r.dispatch_latency).into(),
                pty_read: (&r.pty_read_latency).into(),
                client_write: (&r.client_write_latency).into(),
            },
            contention: JsonContention {
                mutex_contentions: r.contention.mutex_contentions,
                long_holds: r.contention.long_holds,
                channel_overflows: r.contention.channel_overflows,
            },
            slow_ops: r
                .slow_ops
                .iter()
                .map(|op| JsonSlowOp {
                    timestamp_ns: op.timestamp_ns,
                    span_kind: span_kind_display(op.span_kind).to_string(),
                    duration_ns: op.duration_ns,
                })
                .collect(),
        }
    }
}

impl From<&LatencyStats> for JsonLatencyStats {
    fn from(s: &LatencyStats) -> Self {
        Self { p50_us: s.p50_us, p95_us: s.p95_us, p99_us: s.p99_us, count: s.count }
    }
}

/// Format events as a JSON dump.
#[must_use]
pub fn format_dump_json(events: &[FlightEvent]) -> String {
    #[derive(serde::Serialize)]
    struct JsonEvent {
        timestamp_ns: u64,
        span_id: u32,
        event_type: &'static str,
        span_kind: &'static str,
        value: u64,
    }

    let json_events: Vec<_> = events
        .iter()
        .map(|e| JsonEvent {
            timestamp_ns: e.timestamp_ns,
            span_id: e.span_id,
            event_type: match e.event_type {
                EventType::Enter => "enter",
                EventType::Exit => "exit",
                EventType::Event => "event",
                EventType::Panic => "panic",
            },
            span_kind: span_kind_display(e.span_kind),
            value: e.value,
        })
        .collect();

    serde_json::to_string_pretty(&json_events).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flight::{EventType, FlightEvent, SpanKind};

    fn make_exit_event(ts: u64, kind: SpanKind, duration_ns: u64) -> FlightEvent {
        FlightEvent {
            timestamp_ns: ts,
            span_id: 1,
            event_type: EventType::Exit,
            span_kind: kind,
            context: [0; 16],
            value: duration_ns,
        }
    }

    fn make_enter_event(ts: u64, kind: SpanKind) -> FlightEvent {
        FlightEvent {
            timestamp_ns: ts,
            span_id: 1,
            event_type: EventType::Enter,
            span_kind: kind,
            context: [0; 16],
            value: 0,
        }
    }

    #[test]
    fn empty_events_produce_empty_report() {
        let report = build_report(&[], None);
        assert_eq!(report.total_events, 0);
        assert_eq!(report.mutex_latency.count, 0);
        assert_eq!(report.contention.mutex_contentions, 0);
        assert!(report.slow_ops.is_empty());
        assert!(report.uptime.is_none());
    }

    #[test]
    fn single_exit_event_computes_latency() {
        let events = vec![make_exit_event(1_000_000, SpanKind::MutexAcquire, 50_000)]; // 50µs
        let report = build_report(&events, Some(1234));
        assert_eq!(report.mutex_latency.count, 1);
        assert_eq!(report.mutex_latency.p50_us, 50);
        assert_eq!(report.pid, Some(1234));
    }

    #[test]
    fn contention_counted_above_1ms() {
        let events = vec![
            make_exit_event(1_000_000, SpanKind::MutexAcquire, 500_000), // 0.5ms - not contention
            make_exit_event(2_000_000, SpanKind::MutexAcquire, 2_000_000), // 2ms - contention
            make_exit_event(3_000_000, SpanKind::MutexAcquire, 1_500_000), // 1.5ms - contention
        ];
        let report = build_report(&events, None);
        assert_eq!(report.contention.mutex_contentions, 2);
    }

    #[test]
    fn long_holds_counted_above_10ms() {
        let events = vec![
            make_exit_event(1_000_000, SpanKind::MutexAcquire, 5_000_000), // 5ms - not long
            make_exit_event(2_000_000, SpanKind::MutexAcquire, 15_000_000), // 15ms - long
        ];
        let report = build_report(&events, None);
        assert_eq!(report.contention.long_holds, 1);
    }

    #[test]
    fn slow_ops_collected_above_5ms() {
        let events = vec![
            make_exit_event(1_000_000, SpanKind::PtyRead, 3_000_000), // 3ms - not slow
            make_exit_event(2_000_000, SpanKind::IoFlush, 8_000_000), // 8ms - slow
            make_exit_event(3_000_000, SpanKind::PtyRead, 12_000_000), // 12ms - slow
        ];
        let report = build_report(&events, None);
        assert_eq!(report.slow_ops.len(), 2);
        // Most recent first.
        assert_eq!(report.slow_ops[0].timestamp_ns, 3_000_000);
        assert_eq!(report.slow_ops[1].timestamp_ns, 2_000_000);
    }

    #[test]
    fn slow_ops_limited_to_max() {
        let events: Vec<_> = (0..20)
            .map(|i| make_exit_event(i * 1_000_000, SpanKind::PtyRead, 10_000_000))
            .collect();
        let report = build_report(&events, None);
        assert_eq!(report.slow_ops.len(), MAX_SLOW_OPS);
    }

    #[test]
    fn uptime_computed_from_event_range() {
        let events = vec![
            make_enter_event(1_000_000_000, SpanKind::PtyRead), // 1s
            make_exit_event(5_000_000_000, SpanKind::PtyRead, 100), // 5s
        ];
        let report = build_report(&events, None);
        assert_eq!(report.uptime, Some(Duration::from_secs(4)));
    }

    #[test]
    fn percentile_computation_with_multiple_samples() {
        // 100 samples: 1µs to 100µs (in ns: 1000 to 100000)
        let events: Vec<_> = (1..=100)
            .map(|i| make_exit_event(i * 1_000_000, SpanKind::ClientDispatch, i * 1_000))
            .collect();
        let report = build_report(&events, None);
        assert_eq!(report.dispatch_latency.count, 100);
        // p50 should be around 50µs
        assert_eq!(report.dispatch_latency.p50_us, 50);
        // p95 should be around 95µs
        assert_eq!(report.dispatch_latency.p95_us, 95);
        // p99 should be around 99µs
        assert_eq!(report.dispatch_latency.p99_us, 99);
    }

    #[test]
    fn display_format_includes_all_sections() {
        let events = vec![
            make_exit_event(1_000_000_000, SpanKind::MutexAcquire, 50_000),
            make_exit_event(2_000_000_000, SpanKind::PtyRead, 8_000_000),
        ];
        let report = build_report(&events, Some(48291));
        let output = report.to_string();
        assert!(output.contains("rttx-server profiling report"));
        assert!(output.contains("pid: 48291"));
        assert!(output.contains("Latency"));
        assert!(output.contains("Contention"));
        assert!(output.contains("Recent slow operations"));
    }

    #[test]
    fn display_format_no_slow_ops_section_when_empty() {
        let events = vec![make_exit_event(1_000_000, SpanKind::MutexAcquire, 1_000)]; // 1µs
        let report = build_report(&events, None);
        let output = report.to_string();
        assert!(!output.contains("Recent slow operations"));
    }

    #[test]
    fn json_report_serializes_correctly() {
        let events = vec![make_exit_event(1_000_000, SpanKind::MutexAcquire, 50_000)];
        let report = build_report(&events, Some(1234));
        let json: JsonReport = (&report).into();
        let serialized = serde_json::to_string(&json).unwrap();
        assert!(serialized.contains("\"pid\":1234"));
        assert!(serialized.contains("\"mutex_wait\""));
    }

    #[test]
    fn dump_format_includes_all_events() {
        let events = vec![
            make_enter_event(1_000_000, SpanKind::PtyRead),
            make_exit_event(2_000_000, SpanKind::PtyRead, 1_000_000),
        ];
        let dump = format_dump(&events);
        assert!(dump.contains("2 events"));
        assert!(dump.contains("ENTER"));
        assert!(dump.contains("EXIT"));
        assert!(dump.contains("pty.read"));
    }

    #[test]
    fn dump_json_produces_valid_json() {
        let events = vec![
            make_enter_event(1_000_000, SpanKind::PtyRead),
            make_exit_event(2_000_000, SpanKind::PtyRead, 1_000_000),
        ];
        let json_str = format_dump_json(&events);
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().unwrap().len(), 2);
    }

    #[test]
    fn format_duration_hours() {
        assert_eq!(format_duration(Duration::from_mins(134)), "2h 14m");
    }

    #[test]
    fn format_duration_minutes() {
        assert_eq!(format_duration(Duration::from_secs(125)), "2m 05s");
    }

    #[test]
    fn format_duration_seconds() {
        assert_eq!(format_duration(Duration::from_secs(42)), "42s");
    }

    #[test]
    fn format_us_microseconds() {
        assert_eq!(format_us(42), "42 µs");
    }

    #[test]
    fn format_us_milliseconds() {
        assert_eq!(format_us(1_500), "1.5 ms");
    }

    #[test]
    fn format_us_seconds() {
        assert_eq!(format_us(2_500_000), "2.5 s");
    }

    #[test]
    fn context_formatting_nonzero() {
        let mut ctx = [0u8; 16];
        ctx[0] = 0xa3;
        ctx[1] = 0xf2;
        ctx[2] = 0xc1;
        ctx[3] = 0xe0;
        assert_eq!(format_context(&ctx), "id=a3f2c1e0");
    }

    #[test]
    fn context_formatting_zero() {
        let ctx = [0u8; 16];
        assert_eq!(format_context(&ctx), "");
    }

    #[test]
    fn generate_report_from_file() {
        use crate::flight::RingWriter;
        let dir = tempfile::TempDir::new().unwrap();
        let writer = RingWriter::open(dir.path()).unwrap();
        writer.record(&FlightEvent {
            timestamp_ns: 1_000_000_000,
            span_id: 1,
            event_type: EventType::Exit,
            span_kind: SpanKind::MutexAcquire,
            context: [0; 16],
            value: 50_000,
        });
        drop(writer);

        let report = generate_report(&dir.path().join("flight.bin"), Some(999)).unwrap();
        assert_eq!(report.total_events, 1);
        assert_eq!(report.pid, Some(999));
        assert_eq!(report.mutex_latency.count, 1);
    }

    #[test]
    fn generate_report_missing_file_returns_error() {
        let result = generate_report(std::path::Path::new("/nonexistent/flight.bin"), None);
        assert!(result.is_err());
    }

    #[test]
    fn channel_overflow_events_counted() {
        let events = vec![
            FlightEvent {
                timestamp_ns: 1_000_000,
                span_id: 1,
                event_type: EventType::Event,
                span_kind: SpanKind::ChannelSend,
                context: [0; 16],
                value: 4096,
            },
            FlightEvent {
                timestamp_ns: 2_000_000,
                span_id: 2,
                event_type: EventType::Event,
                span_kind: SpanKind::ChannelSend,
                context: [0; 16],
                value: 4096,
            },
        ];
        let report = build_report(&events, None);
        assert_eq!(report.contention.channel_overflows, 2);
    }
}
