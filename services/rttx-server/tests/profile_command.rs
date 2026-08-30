//! Integration test for the profile report generation.
//!
//! Exercises the public profile API from outside the crate to verify
//! report generation from flight recorder files, JSON output, and
//! dump formatting.

use rttx_server::flight::{EventType, FlightEvent, RingWriter, SpanKind};
use rttx_server::profile::{build_report, format_dump, format_dump_json, generate_report};
use tempfile::TempDir;

fn write_sample_events(dir: &std::path::Path) {
    let writer = RingWriter::open(dir).unwrap();
    // Mix of enter/exit events across different span kinds.
    for i in 0..20 {
        writer.record(&FlightEvent {
            timestamp_ns: i * 100_000_000, // 0.1s intervals
            span_id: i as u32,
            event_type: EventType::Enter,
            span_kind: SpanKind::MutexAcquire,
            context: [0; 16],
            value: 0,
        });
        writer.record(&FlightEvent {
            timestamp_ns: i * 100_000_000 + 50_000,
            span_id: i as u32,
            event_type: EventType::Exit,
            span_kind: SpanKind::MutexAcquire,
            context: [0; 16],
            value: (i + 1) * 100_000, // 100µs to 2ms
        });
    }
    // Add a slow operation.
    writer.record(&FlightEvent {
        timestamp_ns: 2_100_000_000,
        span_id: 100,
        event_type: EventType::Exit,
        span_kind: SpanKind::IoFlush,
        context: [0xAB; 16],
        value: 15_000_000, // 15ms
    });
}

#[test]
fn profile_report_from_flight_file() {
    let dir = TempDir::new().unwrap();
    write_sample_events(dir.path());

    let report = generate_report(&dir.path().join("flight.bin"), Some(42)).unwrap();
    assert_eq!(report.pid, Some(42));
    assert!(report.total_events > 0);
    assert!(report.mutex_latency.count > 0);
    assert_eq!(report.slow_ops.len(), 1);
    assert_eq!(report.slow_ops[0].span_kind, SpanKind::IoFlush);
}

#[test]
fn profile_report_display_format_is_human_readable() {
    let dir = TempDir::new().unwrap();
    write_sample_events(dir.path());

    let report = generate_report(&dir.path().join("flight.bin"), Some(1234)).unwrap();
    let output = report.to_string();

    assert!(output.contains("rttx-server profiling report"));
    assert!(output.contains("pid: 1234"));
    assert!(output.contains("Latency"));
    assert!(output.contains("Mutex wait"));
    assert!(output.contains("Contention"));
    assert!(output.contains("Recent slow operations"));
    assert!(output.contains("io.flush"));
}

#[test]
fn profile_json_output_is_valid() {
    let dir = TempDir::new().unwrap();
    write_sample_events(dir.path());

    let report = generate_report(&dir.path().join("flight.bin"), Some(99)).unwrap();
    let json_report: rttx_server::profile::JsonReport = (&report).into();
    let json_str = serde_json::to_string_pretty(&json_report).unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed["pid"], 99);
    assert!(parsed["latency"]["mutex_wait"]["count"].as_u64().unwrap() > 0);
    assert_eq!(parsed["slow_ops"].as_array().unwrap().len(), 1);
}

#[test]
fn profile_dump_includes_all_events() {
    let dir = TempDir::new().unwrap();
    write_sample_events(dir.path());

    let reader = rttx_server::flight::RingReader::open(&dir.path().join("flight.bin")).unwrap();
    let events = reader.read_all();
    let dump = format_dump(&events);

    assert!(dump.contains(&format!("{} events", events.len())));
    assert!(dump.contains("ENTER"));
    assert!(dump.contains("EXIT"));
    assert!(dump.contains("mutex.acquire"));
    assert!(dump.contains("io.flush"));
}

#[test]
fn profile_dump_json_is_valid_array() {
    let dir = TempDir::new().unwrap();
    write_sample_events(dir.path());

    let reader = rttx_server::flight::RingReader::open(&dir.path().join("flight.bin")).unwrap();
    let events = reader.read_all();
    let json_str = format_dump_json(&events);

    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert!(parsed.is_array());
    assert_eq!(parsed.as_array().unwrap().len(), events.len());
}

#[test]
fn profile_last_crash_reads_prev_file() {
    let dir = TempDir::new().unwrap();

    // First instance.
    {
        let writer = RingWriter::open(dir.path()).unwrap();
        writer.record(&FlightEvent {
            timestamp_ns: 1_000_000,
            span_id: 1,
            event_type: EventType::Exit,
            span_kind: SpanKind::PtyRead,
            context: [0; 16],
            value: 500_000,
        });
    }

    // Second instance (renames first to .prev).
    {
        let _writer = RingWriter::open(dir.path()).unwrap();
    }

    // Read the prev file.
    let report = generate_report(&dir.path().join("flight.prev.bin"), None).unwrap();
    assert_eq!(report.total_events, 1);
    assert_eq!(report.pty_read_latency.count, 1);
}

#[test]
fn profile_report_empty_flight_file() {
    let dir = TempDir::new().unwrap();
    let _writer = RingWriter::open(dir.path()).unwrap();

    let report = generate_report(&dir.path().join("flight.bin"), None).unwrap();
    assert_eq!(report.total_events, 0);
    assert!(report.slow_ops.is_empty());
    assert_eq!(report.mutex_latency.count, 0);
}

#[test]
fn build_report_contention_thresholds() {
    let events = vec![
        FlightEvent {
            timestamp_ns: 1_000_000,
            span_id: 1,
            event_type: EventType::Exit,
            span_kind: SpanKind::MutexAcquire,
            context: [0; 16],
            value: 500_000, // 0.5ms — below contention threshold
        },
        FlightEvent {
            timestamp_ns: 2_000_000,
            span_id: 2,
            event_type: EventType::Exit,
            span_kind: SpanKind::MutexAcquire,
            context: [0; 16],
            value: 2_000_000, // 2ms — contention
        },
        FlightEvent {
            timestamp_ns: 3_000_000,
            span_id: 3,
            event_type: EventType::Exit,
            span_kind: SpanKind::MutexAcquire,
            context: [0; 16],
            value: 15_000_000, // 15ms — contention + long hold
        },
    ];

    let report = build_report(&events, None);
    assert_eq!(report.contention.mutex_contentions, 2);
    assert_eq!(report.contention.long_holds, 1);
}
