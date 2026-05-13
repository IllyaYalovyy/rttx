//! Integration test: `ProfilingLayer` records spans to ring buffer and metrics.
//!
//! Verifies end-to-end behavior of the profiling layer when composed with
//! the tracing subscriber registry — spans appear in the ring buffer and
//! the correct histograms are updated.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use rttx_server::flight::{EventType, RingReader, RingWriter, SpanKind};
use rttx_server::metrics::DaemonMetrics;
use rttx_server::profiling::ProfilingLayer;
use tempfile::TempDir;
use tracing_subscriber::layer::SubscriberExt;

#[test]
fn profiling_layer_end_to_end() {
    let dir = TempDir::new().unwrap();
    let metrics = Arc::new(DaemonMetrics::new());
    let ring = Arc::new(RingWriter::open(dir.path()).unwrap());

    let layer = ProfilingLayer::new(Arc::clone(&metrics), Arc::clone(&ring));
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        // Profile span — should be recorded
        let span = tracing::span!(
            target: "rttx_profile",
            tracing::Level::INFO,
            "pty_read_op",
            span_kind = "pty_read"
        );
        {
            let _guard = span.enter();
            std::thread::sleep(std::time::Duration::from_micros(200));
        }

        // Non-profile span — should be ignored
        tracing::info!("regular log message");
        let ignored = tracing::span!(
            target: "rttx_server",
            tracing::Level::INFO,
            "normal_span"
        );
        let _g = ignored.enter();
    });

    // Verify ring buffer
    let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
    let events = reader.read_all();
    assert_eq!(events.len(), 2, "expected Enter + Exit events");
    assert_eq!(events[0].event_type, EventType::Enter);
    assert_eq!(events[0].span_kind, SpanKind::PtyRead);
    assert_eq!(events[1].event_type, EventType::Exit);
    assert_eq!(events[1].span_kind, SpanKind::PtyRead);
    assert!(events[1].value > 0, "exit should carry duration_ns");
    assert!(events[1].timestamp_ns >= events[0].timestamp_ns);

    // Verify metrics
    let snap = metrics.pty_read_latency_us.snapshot();
    let total: u64 = snap.iter().sum();
    assert_eq!(total, 1, "pty_read histogram should have one sample");
}

#[test]
fn profiling_layer_multiple_span_kinds_update_correct_histograms() {
    let dir = TempDir::new().unwrap();
    let metrics = Arc::new(DaemonMetrics::new());
    let ring = Arc::new(RingWriter::open(dir.path()).unwrap());

    let layer = ProfilingLayer::new(Arc::clone(&metrics), Arc::clone(&ring));
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        for kind in &["mutex_acquire", "pty_read", "client_dispatch", "client_write"] {
            let span = tracing::span!(
                target: "rttx_profile",
                tracing::Level::INFO,
                "op",
                span_kind = *kind
            );
            let _guard = span.enter();
        }
    });

    // Each kind should have exactly one sample in its histogram
    let mutex_total: u64 = metrics.mutex_wait_us.snapshot().iter().sum();
    let pty_total: u64 = metrics.pty_read_latency_us.snapshot().iter().sum();
    let dispatch_total: u64 = metrics.dispatch_latency_us.snapshot().iter().sum();
    let write_total: u64 = metrics.client_write_latency_us.snapshot().iter().sum();

    assert_eq!(mutex_total, 1);
    assert_eq!(pty_total, 1);
    assert_eq!(dispatch_total, 1);
    assert_eq!(write_total, 1);

    // Ring buffer should have 8 events (4 enter + 4 exit)
    let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
    assert_eq!(reader.read_all().len(), 8);
}

#[test]
fn profiling_layer_concurrent_spans() {
    use tracing::Dispatch;

    let dir = TempDir::new().unwrap();
    let metrics = Arc::new(DaemonMetrics::new());
    let ring = Arc::new(RingWriter::open(dir.path()).unwrap());

    let layer = ProfilingLayer::new(Arc::clone(&metrics), Arc::clone(&ring));
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = Dispatch::new(subscriber);

    let mut handles = Vec::new();
    for _ in 0..4 {
        let d = dispatch.clone();
        handles.push(std::thread::spawn(move || {
            let _guard = tracing::dispatcher::set_default(&d);
            for _ in 0..100 {
                let span = tracing::span!(
                    target: "rttx_profile",
                    tracing::Level::INFO,
                    "concurrent_op",
                    span_kind = "pty_read"
                );
                let _g = span.enter();
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    // 4 threads × 100 spans = 400 histogram samples
    let snap = metrics.pty_read_latency_us.snapshot();
    let total: u64 = snap.iter().sum();
    assert_eq!(total, 400);

    // 400 spans × 2 events = 800 ring buffer events
    assert_eq!(ring.write_pos(), 800);
}

#[test]
fn profiling_layer_does_not_interfere_with_counters() {
    let dir = TempDir::new().unwrap();
    let metrics = Arc::new(DaemonMetrics::new());
    let ring = Arc::new(RingWriter::open(dir.path()).unwrap());

    // Manually increment a counter before layer usage
    metrics.messages_dispatched.fetch_add(42, Ordering::Relaxed);

    let layer = ProfilingLayer::new(Arc::clone(&metrics), Arc::clone(&ring));
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::span!(
            target: "rttx_profile",
            tracing::Level::INFO,
            "op",
            span_kind = "client_dispatch"
        );
        let _guard = span.enter();
    });

    // Counter should be untouched by the layer
    assert_eq!(metrics.messages_dispatched.load(Ordering::Relaxed), 42);
    // But histogram should have the sample
    let snap = metrics.dispatch_latency_us.snapshot();
    let total: u64 = snap.iter().sum();
    assert_eq!(total, 1);
}
