//! Custom tracing subscriber layer for profiling.
//!
//! Writes span events to the ring buffer flight recorder and updates
//! `DaemonMetrics` latency histograms. Only processes spans with
//! `target = "rttx_profile"` — all other tracing output is ignored.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use tracing::Subscriber;
use tracing::span;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use crate::flight::{EventType, FlightEvent, RingWriter, SpanKind};
use crate::metrics::DaemonMetrics;

/// Target string that profiling spans must use to be recorded.
const PROFILE_TARGET: &str = "rttx_profile";

/// Tracing layer that records span enter/exit events to the ring buffer
/// and updates latency histograms in `DaemonMetrics`.
pub struct ProfilingLayer {
    metrics: Arc<DaemonMetrics>,
    ring: Arc<RingWriter>,
    epoch: Instant,
    next_span_id: AtomicU32,
}

impl ProfilingLayer {
    #[must_use]
    pub fn new(metrics: Arc<DaemonMetrics>, ring: Arc<RingWriter>) -> Self {
        Self { metrics, ring, epoch: Instant::now(), next_span_id: AtomicU32::new(1) }
    }

    fn timestamp_ns(&self) -> u64 {
        self.epoch.elapsed().as_nanos() as u64
    }

    fn allocate_span_id(&self) -> u32 {
        self.next_span_id.fetch_add(1, Ordering::Relaxed)
    }
}

/// Per-span data stored in the tracing registry extensions.
struct SpanData {
    span_id: u32,
    span_kind: SpanKind,
    enter_time: Option<Instant>,
}

/// Parse the `span_kind` field from span attributes.
fn parse_span_kind(attrs: &span::Attributes<'_>) -> SpanKind {
    struct KindVisitor(SpanKind);

    impl tracing::field::Visit for KindVisitor {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            if field.name() == "span_kind" {
                self.0 = match value {
                    "mutex_acquire" => SpanKind::MutexAcquire,
                    "pty_read" => SpanKind::PtyRead,
                    "vte_parse" => SpanKind::VteParse,
                    "client_dispatch" => SpanKind::ClientDispatch,
                    "client_write" => SpanKind::ClientWrite,
                    "channel_send" => SpanKind::ChannelSend,
                    "serialization_tick" => SpanKind::SerializationTick,
                    "io_flush" => SpanKind::IoFlush,
                    "client_session" => SpanKind::ClientSession,
                    _ => SpanKind::Shutdown,
                };
            }
        }

        fn record_debug(&mut self, _field: &tracing::field::Field, _value: &dyn std::fmt::Debug) {}
    }

    let mut visitor = KindVisitor(SpanKind::Shutdown);
    attrs.record(&mut visitor);
    visitor.0
}

impl<S> tracing_subscriber::Layer<S> for ProfilingLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        if attrs.metadata().target() != PROFILE_TARGET {
            return;
        }

        let span_kind = parse_span_kind(attrs);
        let data = SpanData { span_id: self.allocate_span_id(), span_kind, enter_time: None };

        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(data);
        }
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let Some((sid, kind)) = ({
            let mut ext = span.extensions_mut();
            ext.get_mut::<SpanData>().map(|d| {
                d.enter_time = Some(Instant::now());
                (d.span_id, d.span_kind)
            })
        }) else {
            return;
        };

        self.ring.record(&FlightEvent {
            timestamp_ns: self.timestamp_ns(),
            span_id: sid,
            event_type: EventType::Enter,
            span_kind: kind,
            context: [0; 16],
            value: 0,
        });
    }

    fn on_exit(&self, id: &span::Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let Some((sid, kind, duration_ns)) = ({
            let mut ext = span.extensions_mut();
            ext.get_mut::<SpanData>().map(|d| {
                let dur = d.enter_time.map_or(0, |t| t.elapsed().as_nanos() as u64);
                (d.span_id, d.span_kind, dur)
            })
        }) else {
            return;
        };

        let duration_us = duration_ns / 1_000;

        self.ring.record(&FlightEvent {
            timestamp_ns: self.timestamp_ns(),
            span_id: sid,
            event_type: EventType::Exit,
            span_kind: kind,
            context: [0; 16],
            value: duration_ns,
        });

        match kind {
            SpanKind::MutexAcquire => self.metrics.mutex_wait_us.record(duration_us),
            SpanKind::PtyRead => self.metrics.pty_read_latency_us.record(duration_us),
            SpanKind::ClientDispatch => self.metrics.dispatch_latency_us.record(duration_us),
            SpanKind::ClientWrite => self.metrics.client_write_latency_us.record(duration_us),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flight::RingReader;
    use tempfile::TempDir;
    use tracing_subscriber::layer::SubscriberExt;

    fn setup() -> (Arc<DaemonMetrics>, Arc<RingWriter>, TempDir) {
        let dir = TempDir::new().unwrap();
        let metrics = Arc::new(DaemonMetrics::new());
        let ring = Arc::new(RingWriter::open(dir.path()).unwrap());
        (metrics, ring, dir)
    }

    #[test]
    fn profiling_span_records_enter_and_exit() {
        let (metrics, ring, dir) = setup();
        let layer = ProfilingLayer::new(Arc::clone(&metrics), Arc::clone(&ring));

        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::span!(
                target: "rttx_profile",
                tracing::Level::INFO,
                "test_op",
                span_kind = "pty_read"
            );
            let _guard = span.enter();
            std::thread::sleep(std::time::Duration::from_micros(100));
        });

        let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
        let events = reader.read_all();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, EventType::Enter);
        assert_eq!(events[0].span_kind, SpanKind::PtyRead);
        assert_eq!(events[1].event_type, EventType::Exit);
        assert_eq!(events[1].span_kind, SpanKind::PtyRead);
        assert!(events[1].value > 0, "exit event should have non-zero duration");
    }

    #[test]
    fn non_profile_spans_are_ignored() {
        let (metrics, ring, dir) = setup();
        let layer = ProfilingLayer::new(Arc::clone(&metrics), Arc::clone(&ring));

        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::span!(
                target: "some_other_target",
                tracing::Level::INFO,
                "ignored_op",
                span_kind = "pty_read"
            );
            let _guard = span.enter();
        });

        let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
        let events = reader.read_all();
        assert!(events.is_empty());
    }

    #[test]
    fn exit_updates_correct_histogram() {
        let (metrics, ring, _dir) = setup();
        let layer = ProfilingLayer::new(Arc::clone(&metrics), Arc::clone(&ring));

        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::span!(
                target: "rttx_profile",
                tracing::Level::INFO,
                "mutex_op",
                span_kind = "mutex_acquire"
            );
            let _guard = span.enter();
            std::thread::sleep(std::time::Duration::from_micros(50));
        });

        let snap = metrics.mutex_wait_us.snapshot();
        let total: u64 = snap.iter().sum();
        assert_eq!(total, 1, "mutex_wait_us should have exactly one sample");
    }

    #[test]
    fn span_kind_dispatch_updates_dispatch_histogram() {
        let (metrics, ring, _dir) = setup();
        let layer = ProfilingLayer::new(Arc::clone(&metrics), Arc::clone(&ring));

        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::span!(
                target: "rttx_profile",
                tracing::Level::INFO,
                "dispatch",
                span_kind = "client_dispatch"
            );
            let _guard = span.enter();
        });

        let snap = metrics.dispatch_latency_us.snapshot();
        let total: u64 = snap.iter().sum();
        assert_eq!(total, 1);
    }

    #[test]
    fn span_kind_client_write_updates_write_histogram() {
        let (metrics, ring, _dir) = setup();
        let layer = ProfilingLayer::new(Arc::clone(&metrics), Arc::clone(&ring));

        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::span!(
                target: "rttx_profile",
                tracing::Level::INFO,
                "write",
                span_kind = "client_write"
            );
            let _guard = span.enter();
            std::thread::sleep(std::time::Duration::from_micros(10));
        });

        let snap = metrics.client_write_latency_us.snapshot();
        let total: u64 = snap.iter().sum();
        assert_eq!(total, 1);
    }

    #[test]
    fn multiple_spans_accumulate_in_ring_buffer() {
        let (metrics, ring, dir) = setup();
        let layer = ProfilingLayer::new(Arc::clone(&metrics), Arc::clone(&ring));

        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            for _ in 0..5 {
                let span = tracing::span!(
                    target: "rttx_profile",
                    tracing::Level::INFO,
                    "op",
                    span_kind = "io_flush"
                );
                let _guard = span.enter();
            }
        });

        let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
        let events = reader.read_all();
        // 5 spans × 2 events (enter + exit) = 10
        assert_eq!(events.len(), 10);
    }

    #[test]
    fn span_ids_are_sequential() {
        let (metrics, ring, dir) = setup();
        let layer = ProfilingLayer::new(Arc::clone(&metrics), Arc::clone(&ring));

        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            for _ in 0..3 {
                let span = tracing::span!(
                    target: "rttx_profile",
                    tracing::Level::INFO,
                    "op",
                    span_kind = "pty_read"
                );
                let _guard = span.enter();
            }
        });

        let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
        let events = reader.read_all();
        // Enter events at indices 0, 2, 4 should have sequential span_ids
        assert_eq!(events[0].span_id, 1);
        assert_eq!(events[2].span_id, 2);
        assert_eq!(events[4].span_id, 3);
    }

    #[test]
    fn unknown_span_kind_defaults_to_shutdown() {
        let (metrics, ring, dir) = setup();
        let layer = ProfilingLayer::new(Arc::clone(&metrics), Arc::clone(&ring));

        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::span!(
                target: "rttx_profile",
                tracing::Level::INFO,
                "op",
                span_kind = "unknown_kind"
            );
            let _guard = span.enter();
        });

        let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
        let events = reader.read_all();
        assert_eq!(events[0].span_kind, SpanKind::Shutdown);
    }
}
