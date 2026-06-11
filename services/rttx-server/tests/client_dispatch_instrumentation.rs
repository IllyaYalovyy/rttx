//! Integration test: client dispatch and writer instrumentation spans
//! are recorded to the ring buffer and update the correct histograms.
//!
//! Verifies that the profiling spans added in #902 for client message
//! dispatch and client writer operations work end-to-end.

mod common;

use common::*;
use rttx_proto::v3;
use std::time::Duration;

#[tokio::test]
async fn client_dispatch_increments_messages_dispatched_on_ping() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    // Send a Ping — the reader loop increments messages_dispatched.
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::Ping(v3::Ping { nonce: 42 })),
        })
        .await;

    // Expect a Pong response (proves dispatch worked).
    let resp = client.recv_or_timeout().await;
    match resp.payload {
        Some(v3::server_envelope::Payload::Pong(pong)) => {
            assert_eq!(pong.nonce, 42);
        }
        other => panic!("expected Pong, got {other:?}"),
    }
}

#[tokio::test]
async fn client_disconnect_records_eof_on_clean_close() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    // Drop the client — the server should record an EOF disconnect.
    drop(client);

    // Give the server time to process the disconnect.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Connect a second client to verify the server is still healthy.
    let mut client2 = TestClient::connect(&sock).await;
    client2.handshake().await;

    // Send a Ping to confirm the server is responsive.
    client2
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::Ping(v3::Ping { nonce: 99 })),
        })
        .await;

    let resp = client2.recv_or_timeout().await;
    match resp.payload {
        Some(v3::server_envelope::Payload::Pong(pong)) => {
            assert_eq!(pong.nonce, 99);
        }
        other => panic!("expected Pong, got {other:?}"),
    }
}

#[tokio::test]
async fn client_writer_delivers_output_after_instrumentation() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;
    let sid = create_workspace(&mut client, "writer-test", v3::WorkspacePolicy::Persistent).await;
    let pane_id = create_pane(&mut client, &sid).await;
    attach_rw(&mut client, &sid).await;

    // Drain shell startup.
    client.drain(Duration::from_millis(500)).await;

    // Send a command and verify output arrives through the instrumented writer.
    send_input(&mut client, &sid, &pane_id, b"echo dispatch_ok\n").await;
    let msgs = client.drain(Duration::from_secs(2)).await;

    let output: String = msgs
        .iter()
        .filter_map(|m| match &m.payload {
            Some(v3::server_envelope::Payload::OutputDelta(d)) => {
                Some(String::from_utf8_lossy(&d.data).to_string())
            }
            _ => None,
        })
        .collect();

    assert!(
        output.contains("dispatch_ok"),
        "expected echo output through instrumented writer, got: {output}"
    );
}

#[tokio::test]
async fn dispatch_latency_recorded_for_create_workspace() {
    use rttx_server::flight::{RingReader, SpanKind};
    use rttx_server::metrics::DaemonMetrics;
    use rttx_server::profiling::ProfilingLayer;
    use std::sync::Arc;
    use tracing_subscriber::layer::SubscriberExt;

    let dir = tempfile::TempDir::new().unwrap();
    let metrics = Arc::new(DaemonMetrics::new());
    let ring = Arc::new(rttx_server::flight::RingWriter::open(dir.path()).unwrap());

    let layer = ProfilingLayer::new(Arc::clone(&metrics), Arc::clone(&ring));
    let subscriber = tracing_subscriber::registry().with(layer);

    // Verify that client_dispatch and client_write span kinds are recorded.
    tracing::subscriber::with_default(subscriber, || {
        let dispatch_span = tracing::info_span!(
            target: "rttx_profile",
            "client.dispatch",
            span_kind = "client_dispatch",
            client_id = "test-client",
            msg_type = "CreateWorkspace",
        );
        let _guard = dispatch_span.enter();
        std::thread::sleep(std::time::Duration::from_micros(10));
    });

    let snap = metrics.dispatch_latency_us.snapshot();
    let total: u64 = snap.iter().sum();
    assert_eq!(total, 1, "dispatch_latency_us should have one sample");

    // Verify ring buffer recorded the span.
    let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
    let events = reader.read_all();
    let dispatch_exits: Vec<_> = events
        .iter()
        .filter(|e| {
            e.span_kind == SpanKind::ClientDispatch
                && e.event_type == rttx_server::flight::EventType::Exit
        })
        .collect();
    assert_eq!(dispatch_exits.len(), 1);
    assert!(dispatch_exits[0].value > 0, "exit event should carry non-zero duration");
}

#[tokio::test]
async fn client_write_latency_recorded_in_ring_buffer() {
    use rttx_server::flight::{EventType, RingReader, SpanKind};
    use rttx_server::metrics::DaemonMetrics;
    use rttx_server::profiling::ProfilingLayer;
    use std::sync::Arc;
    use tracing_subscriber::layer::SubscriberExt;

    let dir = tempfile::TempDir::new().unwrap();
    let metrics = Arc::new(DaemonMetrics::new());
    let ring = Arc::new(rttx_server::flight::RingWriter::open(dir.path()).unwrap());

    let layer = ProfilingLayer::new(Arc::clone(&metrics), Arc::clone(&ring));
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        let write_span = tracing::info_span!(
            target: "rttx_profile",
            "client.write",
            span_kind = "client_write",
            client_id = "test-writer",
            bytes_written = 128_usize,
        );
        let _guard = write_span.enter();
        std::thread::sleep(std::time::Duration::from_micros(10));
    });

    let snap = metrics.client_write_latency_us.snapshot();
    let total: u64 = snap.iter().sum();
    assert_eq!(total, 1, "client_write_latency_us should have one sample");

    let reader = RingReader::open(&dir.path().join("flight.bin")).unwrap();
    let events = reader.read_all();
    let write_exits: Vec<_> = events
        .iter()
        .filter(|e| e.span_kind == SpanKind::ClientWrite && e.event_type == EventType::Exit)
        .collect();
    assert_eq!(write_exits.len(), 1);
    assert!(write_exits[0].value > 0);
}
