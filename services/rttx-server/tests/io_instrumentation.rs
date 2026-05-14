//! Integration test: I/O instrumentation spans for PTY read, VTE parse,
//! serialization tick, and IO flush are recorded to the ring buffer and
//! update the correct histograms.

use std::sync::Arc;

use rttx_server::flight::{EventType, RingReader, RingWriter, SpanKind};
use rttx_server::metrics::DaemonMetrics;
use rttx_server::profiling::ProfilingLayer;
use tempfile::TempDir;
use tracing_subscriber::layer::SubscriberExt;

#[test]
fn io_span_kinds_update_correct_histograms() {
    let dir = TempDir::new().unwrap();
    let metrics = Arc::new(DaemonMetrics::new());
    let ring = Arc::new(RingWriter::open(dir.path()).unwrap());

    let layer = ProfilingLayer::new(Arc::clone(&metrics), Arc::clone(&ring));
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        for kind in &["vte_parse", "serialization_tick", "io_flush"] {
            let span = tracing::span!(
                target: "rttx_profile",
                tracing::Level::INFO,
                "io_op",
                span_kind = *kind
            );
            let _guard = span.enter();
            std::thread::sleep(std::time::Duration::from_micros(10));
        }
    });

    let vte: u64 = metrics.vte_parse_latency_us.snapshot().iter().sum();
    let ser: u64 = metrics.serialization_tick_latency_us.snapshot().iter().sum();
    let flush: u64 = metrics.io_flush_latency_us.snapshot().iter().sum();

    assert_eq!(vte, 1, "vte_parse histogram should have one sample");
    assert_eq!(ser, 1, "serialization_tick histogram should have one sample");
    assert_eq!(flush, 1, "io_flush histogram should have one sample");

    // Ring buffer: 3 spans × 2 events (enter + exit)
    let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
    let events = reader.read_all();
    assert_eq!(events.len(), 6);

    let exit_events: Vec<_> = events.iter().filter(|e| e.event_type == EventType::Exit).collect();
    assert_eq!(exit_events.len(), 3);
    assert_eq!(exit_events[0].span_kind, SpanKind::VteParse);
    assert_eq!(exit_events[1].span_kind, SpanKind::SerializationTick);
    assert_eq!(exit_events[2].span_kind, SpanKind::IoFlush);

    // Each exit event should carry a non-zero duration
    for ev in &exit_events {
        assert!(ev.value > 0, "exit event for {:?} should have duration", ev.span_kind);
    }
}

#[test]
fn io_instrumentation_does_not_affect_other_histograms() {
    let dir = TempDir::new().unwrap();
    let metrics = Arc::new(DaemonMetrics::new());
    let ring = Arc::new(RingWriter::open(dir.path()).unwrap());

    let layer = ProfilingLayer::new(Arc::clone(&metrics), Arc::clone(&ring));
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::span!(
            target: "rttx_profile",
            tracing::Level::INFO,
            "io_op",
            span_kind = "io_flush"
        );
        let _guard = span.enter();
    });

    // Only io_flush should be updated
    let flush: u64 = metrics.io_flush_latency_us.snapshot().iter().sum();
    assert_eq!(flush, 1);

    let pty: u64 = metrics.pty_read_latency_us.snapshot().iter().sum();
    let vte: u64 = metrics.vte_parse_latency_us.snapshot().iter().sum();
    let ser: u64 = metrics.serialization_tick_latency_us.snapshot().iter().sum();
    let mutex: u64 = metrics.mutex_wait_us.snapshot().iter().sum();
    assert_eq!(pty, 0);
    assert_eq!(vte, 0);
    assert_eq!(ser, 0);
    assert_eq!(mutex, 0);
}
