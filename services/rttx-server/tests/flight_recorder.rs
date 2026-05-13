//! Integration test for the flight recorder ring buffer.
//!
//! Exercises the public API from outside the crate to verify the
//! write/read contract, crash persistence, and stale file handling.

use rttx_server::flight::{EventType, FlightEvent, RingReader, RingWriter, SpanKind};
use tempfile::TempDir;

const fn make_event(ts: u64, span_id: u32, kind: SpanKind) -> FlightEvent {
    FlightEvent {
        timestamp_ns: ts,
        span_id,
        event_type: EventType::Enter,
        span_kind: kind,
        context: [0; 16],
        value: ts,
    }
}

#[test]
fn flight_recorder_survives_writer_drop() {
    let dir = TempDir::new().unwrap();

    // Write events then drop (simulates kill -9)
    {
        let writer = RingWriter::open(dir.path()).unwrap();
        for i in 0..50 {
            writer.record(&make_event(i * 1000, i as u32, SpanKind::PtyRead));
        }
    }

    // Independent reader can recover all events
    let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
    let events = reader.read_all();
    assert_eq!(events.len(), 50);
    assert_eq!(events[0].timestamp_ns, 0);
    assert_eq!(events[49].timestamp_ns, 49_000);
}

#[test]
fn flight_recorder_previous_instance_preserved() {
    let dir = TempDir::new().unwrap();

    // First daemon instance
    {
        let writer = RingWriter::open(dir.path()).unwrap();
        writer.record(&make_event(100, 1, SpanKind::Shutdown));
    }

    // Second daemon instance — old file becomes .prev
    {
        let writer = RingWriter::open(dir.path()).unwrap();
        writer.record(&make_event(200, 2, SpanKind::ClientSession));
    }

    // Verify both files are readable
    let prev = RingReader::open(&dir.path().join("flight.prev.bin")).unwrap();
    assert_eq!(prev.read_all()[0].timestamp_ns, 100);

    let current = RingReader::open(&dir.path().join("flight.bin")).unwrap();
    assert_eq!(current.read_all()[0].timestamp_ns, 200);
}

#[test]
fn flight_recorder_wrap_around_full_ring() {
    let dir = TempDir::new().unwrap();
    let writer = RingWriter::open(dir.path()).unwrap();

    // Write 65,536 + 500 events to wrap around
    let total: u64 = 65_536 + 500;
    for i in 0..total {
        writer.record(&make_event(i, i as u32, SpanKind::IoFlush));
    }

    let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
    let events = reader.read_all();

    // Should contain exactly 65,536 events
    assert_eq!(events.len(), 65_536);

    // Oldest should be event 500 (first 500 overwritten)
    assert_eq!(events[0].timestamp_ns, 500);

    // Newest should be the last written
    assert_eq!(events[65_535].timestamp_ns, total - 1);

    // All events in chronological order
    for window in events.windows(2) {
        assert!(window[1].timestamp_ns > window[0].timestamp_ns);
    }
}
