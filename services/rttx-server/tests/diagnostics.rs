//! Integration tests for the `GetDiagnostics` protocol message.

mod common;

use common::*;
use rttx_proto::v3;

#[tokio::test]
async fn diagnostics_empty_server() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::GetDiagnostics(v3::GetDiagnostics {})),
        })
        .await;

    let resp = client.recv_or_timeout().await;
    let report = match resp.payload {
        Some(v3::server_envelope::Payload::DiagnosticsReport(r)) => r,
        other => panic!("expected DiagnosticsReport, got {other:?}"),
    };

    assert_eq!(report.workspace_count, 0);
    assert_eq!(report.total_pane_count, 0);
    assert_eq!(report.total_active_panes, 0);
    assert_eq!(report.total_exited_panes, 0);
    assert_eq!(report.total_raw_bytes, 0);
    assert_eq!(report.total_pending_flush, 0);
    assert!(report.workspaces.is_empty());
}

#[tokio::test]
async fn diagnostics_with_session_and_pane() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let sid = create_workspace(&mut client, "diag-test", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &sid).await;
    let _pane_id = create_pane(&mut client, &sid).await;

    // Let the pane produce some output.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    client.drain(std::time::Duration::from_millis(200)).await;

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::GetDiagnostics(v3::GetDiagnostics {})),
        })
        .await;

    let resp = loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::DiagnosticsReport(r)) => break r,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected DiagnosticsReport, got {other:?}"),
        }
    };

    assert_eq!(resp.workspace_count, 1);
    assert!(resp.total_pane_count >= 1);
    assert!(resp.total_active_panes >= 1);
    assert_eq!(resp.workspaces.len(), 1);
    assert_eq!(resp.workspaces[0].name, "diag-test");
    assert!(!resp.workspaces[0].panes.is_empty());
}

#[tokio::test]
async fn diagnostics_reflects_cleanup_after_terminate() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let sid = create_workspace(&mut client, "cleanup", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &sid).await;
    let _pane_id = create_pane(&mut client, &sid).await;

    // Verify non-zero state.
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::GetDiagnostics(v3::GetDiagnostics {})),
        })
        .await;
    let before = loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::DiagnosticsReport(r)) => break r,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected DiagnosticsReport, got {other:?}"),
        }
    };
    assert_eq!(before.workspace_count, 1);

    // Terminate and verify cleanup.
    terminate_workspace(&mut client, &sid).await;

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::GetDiagnostics(v3::GetDiagnostics {})),
        })
        .await;
    let after = loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::DiagnosticsReport(r)) => break r,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected DiagnosticsReport, got {other:?}"),
        }
    };
    assert_eq!(after.workspace_count, 0, "terminated session must be cleaned up");
    assert_eq!(after.total_pane_count, 0);
    assert_eq!(after.total_raw_bytes, 0);
    assert_eq!(after.total_pending_flush, 0);
}

#[tokio::test]
async fn diagnostics_workspace_reports_current_pane_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let sid = create_workspace(&mut client, "diag-fields", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &sid).await;
    let _pane = create_pane(&mut client, &sid).await;
    client.drain(std::time::Duration::from_millis(200)).await;

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

    // The workspace diagnostics carry exactly the current field set (active /
    // exited pane counts and attached-client count) — one attached client and
    // one live pane.
    assert_eq!(report.workspaces.len(), 1);
    let ws = &report.workspaces[0];
    assert_eq!(ws.name, "diag-fields");
    assert!(ws.active_pane_count >= 1);
    assert_eq!(ws.attached_client_count, 1);
    assert!(!ws.panes.is_empty());
}

#[tokio::test]
async fn diagnostics_totals_track_multiple_workspaces() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let a = create_workspace(&mut client, "diag-a", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &a).await;
    let _pa = create_pane(&mut client, &a).await;

    let b = create_workspace(&mut client, "diag-b", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &b).await;
    let _pb = create_pane(&mut client, &b).await;

    client.drain(std::time::Duration::from_millis(200)).await;

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

    assert_eq!(report.workspace_count, 2);
    assert!(report.total_pane_count >= 2);
    assert_eq!(report.workspaces.len(), 2);
}
