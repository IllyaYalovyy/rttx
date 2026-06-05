//! Tests verifying `HashMap` cleanup and bounded data structure caps.
//!
//! These tests use the diagnostics API to verify that server-internal
//! data structures are properly cleaned up after session/pane lifecycle
//! operations and that bounded structures respect their limits.

mod common;

use common::*;
use rttx_proto::v3;

/// After creating and terminating multiple sessions, the server's internal
/// hash maps (sessions, `pty_writers`, `client_senders`) must return to zero.
#[tokio::test]
async fn hashmap_cleanup_after_session_terminate() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    // Create several sessions with panes, then terminate them all.
    for i in 0..5 {
        let sid =
            create_runtime(&mut client, &format!("map-{i}"), v3::RuntimePolicy::Persistent).await;
        attach_rw(&mut client, &sid).await;
        let _pane = create_pane(&mut client, &sid).await;
        terminate_runtime(&mut client, &sid).await;
    }

    // Diagnostics should show zero sessions and zero panes.
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::GetDiagnostics(v3::GetDiagnostics {})),
        })
        .await;
    let report = loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::DiagnosticsReport(r)) => break r,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected DiagnosticsReport, got {other:?}"),
        }
    };

    assert_eq!(report.runtime_count, 0, "sessions HashMap must be empty after terminate");
    assert_eq!(report.total_pane_count, 0, "no panes should remain");
    assert_eq!(report.pty_writer_count, 0, "pty_writers HashMap must be empty");
}

/// After closing all panes in a session, the pane hash map within the
/// session must be empty.
#[tokio::test]
async fn hashmap_cleanup_after_pane_close() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let sid = create_runtime(&mut client, "pane-cleanup", v3::RuntimePolicy::Persistent).await;
    attach_rw(&mut client, &sid).await;

    // Create and close several panes.
    for _ in 0..5 {
        let pane_id = create_pane(&mut client, &sid).await;
        close_pane(&mut client, &sid, &pane_id).await;
    }

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::GetDiagnostics(v3::GetDiagnostics {})),
        })
        .await;
    let report = loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::DiagnosticsReport(r)) => break r,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected DiagnosticsReport, got {other:?}"),
        }
    };

    assert_eq!(report.runtime_count, 1);
    assert_eq!(report.runtimes[0].active_pane_count, 0, "all panes must be cleaned up");
    assert_eq!(report.runtimes[0].exited_pane_count, 0, "no exited panes should linger");
}

/// Ephemeral sessions must be fully cleaned up after the last client detaches.
#[tokio::test]
async fn hashmap_cleanup_ephemeral_detach() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    for i in 0..3 {
        let sid =
            create_runtime(&mut client, &format!("eph-{i}"), v3::RuntimePolicy::Ephemeral).await;
        attach_rw(&mut client, &sid).await;
        detach_runtime(&mut client, &sid).await;
    }

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::GetDiagnostics(v3::GetDiagnostics {})),
        })
        .await;
    let report = loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::DiagnosticsReport(r)) => break r,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected DiagnosticsReport, got {other:?}"),
        }
    };

    assert_eq!(report.runtime_count, 0, "ephemeral sessions must be cleaned up on detach");
    assert_eq!(report.total_pane_count, 0);
}

/// Client disconnect must clean up the `client_senders` hash map entry.
#[tokio::test]
async fn client_sender_cleanup_on_disconnect() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    // Connect and disconnect several clients.
    for _ in 0..5 {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        drop(c);
    }

    // Small delay for the server to process disconnects.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Connect a fresh client and check diagnostics.
    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::GetDiagnostics(v3::GetDiagnostics {})),
        })
        .await;
    let report = loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::DiagnosticsReport(r)) => break r,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected DiagnosticsReport, got {other:?}"),
        }
    };

    // Only the current client should be connected.
    assert_eq!(report.client_count, 1, "disconnected clients must be cleaned up");
}
