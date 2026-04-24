//! Integration tests for the `GetDiagnostics` protocol message.

mod common;

use common::*;
use rttx_proto::proto;

#[tokio::test]
async fn diagnostics_empty_server() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::GetDiagnostics(proto::GetDiagnostics {})),
        })
        .await;

    let resp = client.recv_or_timeout().await;
    let report = match resp.msg {
        Some(proto::server_message::Msg::DiagnosticsReport(r)) => r,
        other => panic!("expected DiagnosticsReport, got {other:?}"),
    };

    assert_eq!(report.runtime_count, 0);
    assert_eq!(report.total_pane_count, 0);
    assert_eq!(report.total_active_panes, 0);
    assert_eq!(report.total_exited_panes, 0);
    assert_eq!(report.total_raw_bytes, 0);
    assert_eq!(report.total_pending_flush, 0);
    assert!(report.runtimes.is_empty());
}

#[tokio::test]
async fn diagnostics_with_session_and_pane() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let sid = create_runtime(&mut client, "diag-test", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut client, &sid).await;
    let _pane_id = create_pane(&mut client, &sid).await;

    // Let the pane produce some output.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    client.drain(std::time::Duration::from_millis(200)).await;

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::GetDiagnostics(proto::GetDiagnostics {})),
        })
        .await;

    let resp = loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::DiagnosticsReport(r)) => break r,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected DiagnosticsReport, got {other:?}"),
        }
    };

    assert_eq!(resp.runtime_count, 1);
    assert!(resp.total_pane_count >= 1);
    assert!(resp.total_active_panes >= 1);
    assert_eq!(resp.runtimes.len(), 1);
    assert_eq!(resp.runtimes[0].name, "diag-test");
    assert!(!resp.runtimes[0].panes.is_empty());
}

#[tokio::test]
async fn diagnostics_reflects_cleanup_after_terminate() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let sid = create_runtime(&mut client, "cleanup", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut client, &sid).await;
    let _pane_id = create_pane(&mut client, &sid).await;

    // Verify non-zero state.
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::GetDiagnostics(proto::GetDiagnostics {})),
        })
        .await;
    let before = loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::DiagnosticsReport(r)) => break r,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected DiagnosticsReport, got {other:?}"),
        }
    };
    assert_eq!(before.runtime_count, 1);

    // Terminate and verify cleanup.
    terminate_runtime(&mut client, &sid).await;

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::GetDiagnostics(proto::GetDiagnostics {})),
        })
        .await;
    let after = loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::DiagnosticsReport(r)) => break r,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected DiagnosticsReport, got {other:?}"),
        }
    };
    assert_eq!(after.runtime_count, 0, "terminated session must be cleaned up");
    assert_eq!(after.total_pane_count, 0);
    assert_eq!(after.total_raw_bytes, 0);
    assert_eq!(after.total_pending_flush, 0);
}
