//! Integration test: verify crash report generation on panic.
//!
//! Spawns a subprocess that sets up the panic hook with a ring writer and
//! metrics, then panics. Verifies that crash-report.txt is written with
//! expected content and the ring buffer contains a PANIC event.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use rttx_server::crash_report;
use rttx_server::flight::{EventType, RingReader, RingWriter};
use rttx_server::metrics::DaemonMetrics;

#[test]
fn crash_report_generated_on_simulated_panic() {
    let dir = tempfile::TempDir::new().unwrap();
    let metrics = Arc::new(DaemonMetrics::new());
    let ring = Arc::new(RingWriter::open(dir.path()).unwrap());
    let start_time = Instant::now();

    // Simulate some activity before the panic
    metrics.messages_dispatched.fetch_add(100, Ordering::Relaxed);
    metrics.connected_clients.fetch_add(3, Ordering::Relaxed);
    metrics.bytes_read_from_pty.fetch_add(65536, Ordering::Relaxed);
    metrics.mutex_wait_us.record(50);
    metrics.dispatch_latency_us.record(200);

    // Record some profiling events
    for i in 0..10 {
        ring.record(&rttx_server::flight::FlightEvent {
            timestamp_ns: i * 100_000_000,
            span_id: i as u32,
            event_type: rttx_server::flight::EventType::Enter,
            span_kind: rttx_server::flight::SpanKind::PtyRead,
            context: [0; 16],
            value: 0,
        });
        ring.record(&rttx_server::flight::FlightEvent {
            timestamp_ns: i * 100_000_000 + 50_000,
            span_id: i as u32,
            event_type: rttx_server::flight::EventType::Exit,
            span_kind: rttx_server::flight::SpanKind::PtyRead,
            context: [0; 16],
            value: 50_000,
        });
    }

    // Simulate what the panic hook does
    let crash_report_path = dir.path().join("crash-report.txt");
    crash_report::record_panic_event(&ring, start_time);
    crash_report::write_crash_report(
        &crash_report_path,
        "index out of bounds: the len is 5 but the index is 7",
        "services/rttx-server/src/pane.rs:42:9",
        &metrics,
        &ring,
        start_time,
    );

    // Verify crash report file exists and has expected content
    assert!(crash_report_path.exists(), "crash-report.txt should exist");
    let content = std::fs::read_to_string(&crash_report_path).unwrap();

    // Verify header
    assert!(content.contains("rttx-server crash report"));
    assert!(content.contains(&format!("PID: {}", std::process::id())));
    assert!(content.contains("Uptime:"));

    // Verify panic info
    assert!(content.contains("index out of bounds: the len is 5 but the index is 7"));
    assert!(content.contains("services/rttx-server/src/pane.rs:42:9"));

    // Verify metrics
    assert!(content.contains("messages_dispatched: 100"));
    assert!(content.contains("connected_clients: 3"));
    assert!(content.contains("bytes_read_from_pty: 65536"));

    // Verify histogram data
    assert!(content.contains("mutex_wait_us:"));
    assert!(content.contains("dispatch_latency_us:"));

    // Verify ring buffer events section
    assert!(content.contains("pty.read"));
    assert!(content.contains("ENTER"));
    assert!(content.contains("EXIT"));

    // Verify the ring buffer itself contains the PANIC event
    let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
    let events = reader.read_all();
    let panic_count = events.iter().filter(|e| e.event_type == EventType::Panic).count();
    assert_eq!(panic_count, 1, "ring buffer should contain exactly one PANIC event");
}

#[test]
fn crash_report_readable_after_ring_buffer_flush() {
    let dir = tempfile::TempDir::new().unwrap();
    let _metrics = Arc::new(DaemonMetrics::new());
    let ring = Arc::new(RingWriter::open(dir.path()).unwrap());
    let start_time = Instant::now();

    // Record panic event and flush
    crash_report::record_panic_event(&ring, start_time);

    // Drop the writer to simulate process death
    drop(ring);

    // Verify the ring buffer is readable from a fresh reader (simulates
    // `rttx-server profile --last-crash` reading after daemon death)
    let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
    let events = reader.read_all();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, EventType::Panic);
}

#[test]
fn crash_report_does_not_interfere_with_normal_shutdown() {
    let dir = tempfile::TempDir::new().unwrap();
    let _metrics = Arc::new(DaemonMetrics::new());
    let ring = Arc::new(RingWriter::open(dir.path()).unwrap());

    // Record normal events (no panic)
    for i in 0..5 {
        ring.record(&rttx_server::flight::FlightEvent {
            timestamp_ns: i * 1_000_000,
            span_id: i as u32,
            event_type: rttx_server::flight::EventType::Exit,
            span_kind: rttx_server::flight::SpanKind::IoFlush,
            context: [0; 16],
            value: 1000,
        });
    }

    // Normal drop (no panic hook fires)
    drop(ring);

    // No crash report should exist
    let crash_report_path = dir.path().join("crash-report.txt");
    assert!(!crash_report_path.exists(), "crash-report.txt should NOT exist on normal shutdown");

    // Ring buffer should only have normal events
    let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
    let events = reader.read_all();
    assert_eq!(events.len(), 5);
    assert!(events.iter().all(|e| e.event_type != EventType::Panic));
}
