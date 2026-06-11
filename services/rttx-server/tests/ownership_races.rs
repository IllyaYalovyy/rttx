//! Multi-client ownership race integration tests.
//!
//! Exercises concurrent access patterns: competing writer attaches,
//! read-only clients during mutations, detach-vs-terminate races,
//! and writer disconnect during pane operations.

mod common;

use common::*;
use rttx_proto::v3;
use std::time::Duration;

// ── Helpers ─────────────────────────────────────────────────────

// ── Competing writer attaches ───────────────────────────────────

#[tokio::test]
async fn three_competing_writers_only_first_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c1 = TestClient::connect(&sock).await;
    c1.handshake().await;
    let runtime_id = create_workspace(&mut c1, "race", v3::WorkspacePolicy::Persistent).await;
    let snap = attach_rw(&mut c1, &runtime_id).await;
    assert_eq!(snap.client_role, v3::WorkspaceClientRole::Writer as i32);

    for i in 0..2 {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        c.send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
            })),
        })
        .await;
        match c.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::AttachBlocked(b)) => {
                assert_eq!(b.attached_client_count, 1, "client {i}: wrong attach count");
            }
            other => panic!("client {i}: expected AttachBlocked, got {other:?}"),
        }
    }
}

// ── Read-only clients during active mutation ────────────────────

#[tokio::test]
async fn readers_observe_pane_created_push() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let runtime_id =
        create_workspace(&mut writer, "push-test", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut writer, &runtime_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    attach_ro(&mut reader, &runtime_id).await;

    // Writer creates a pane.
    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
                runtime_id: runtime_id.clone(),
                cwd: None,
                dark_background: None,
                cols: 0,
                rows: 0,
                no_persist: None,
            })),
        })
        .await;

    // Writer gets PaneCreated response.
    let writer_resp = writer.recv_or_timeout().await;
    assert!(
        matches!(writer_resp.payload, Some(v3::server_envelope::Payload::PaneCreated(_))),
        "writer should get PaneCreated"
    );

    // Reader receives Delta pushes from the new pane's PTY output.
    let reader_msgs = reader.drain(Duration::from_secs(2)).await;
    assert!(
        reader_msgs
            .iter()
            .any(|m| matches!(m.payload, Some(v3::server_envelope::Payload::OutputDelta(_)))),
        "reader should receive Delta pushes from the new pane"
    );
}

#[tokio::test]
async fn multiple_readers_see_consistent_revision() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let runtime_id =
        create_workspace(&mut writer, "rev-test", v3::WorkspacePolicy::Persistent).await;
    let snap = attach_rw(&mut writer, &runtime_id).await;
    let base_rev = snap.workspace_revision;

    let mut r1 = TestClient::connect(&sock).await;
    r1.handshake().await;
    let s1 = attach_ro(&mut r1, &runtime_id).await;

    let mut r2 = TestClient::connect(&sock).await;
    r2.handshake().await;
    let s2 = attach_ro(&mut r2, &runtime_id).await;

    // Each reader attach bumps revision.
    assert!(s1.workspace_revision > base_rev);
    assert!(s2.workspace_revision > s1.workspace_revision);

    // Inventory should show consistent counts.
    let workspaces = list_workspaces(&mut r2).await;
    assert_eq!(workspaces[0].read_only_client_count, 2);
    assert!(workspaces[0].has_write_owner);
}

// ── Detach vs terminate races ───────────────────────────────────

#[tokio::test]
async fn writer_detach_then_reader_detach_leaves_clean_state() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let runtime_id =
        create_workspace(&mut writer, "detach-race", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut writer, &runtime_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    attach_ro(&mut reader, &runtime_id).await;

    // Writer detaches first.
    detach_workspace(&mut writer, &runtime_id).await;
    // Reader gets WorkspaceDetached push.
    reader.drain(Duration::from_millis(200)).await;

    // Reader detaches.
    detach_workspace(&mut reader, &runtime_id).await;

    // Session should still exist (persistent policy).
    let mut checker = TestClient::connect(&sock).await;
    checker.handshake().await;
    let workspaces = list_workspaces(&mut checker).await;
    assert_eq!(workspaces.len(), 1);
    assert!(!workspaces[0].has_write_owner);
    assert_eq!(workspaces[0].read_only_client_count, 0);
}

#[tokio::test]
async fn terminate_while_reader_attached_notifies_reader() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let runtime_id =
        create_workspace(&mut writer, "term-race", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut writer, &runtime_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    attach_ro(&mut reader, &runtime_id).await;

    // Writer terminates.
    writer
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::TerminateWorkspace(
                v3::TerminateWorkspace { runtime_id: runtime_id.clone() },
            )),
        })
        .await;

    // Both should get WorkspaceTerminated.
    let w_resp = writer.recv_or_timeout().await;
    assert!(matches!(w_resp.payload, Some(v3::server_envelope::Payload::WorkspaceTerminated(_))));

    let r_resp = reader.recv_or_timeout().await;
    assert!(matches!(r_resp.payload, Some(v3::server_envelope::Payload::WorkspaceTerminated(_))));

    // Session gone.
    let mut checker = TestClient::connect(&sock).await;
    checker.handshake().await;
    let workspaces = list_workspaces(&mut checker).await;
    assert!(workspaces.is_empty());
}

// ── Writer disconnect during pane operations ────────────────────

#[tokio::test]
async fn writer_disconnect_frees_ownership_for_new_writer() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let runtime_id =
        create_workspace(&mut writer, "disconnect", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut writer, &runtime_id).await;

    // Drop the writer (simulates disconnect).
    drop(writer);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // New client should be able to attach as writer.
    let mut new_writer = TestClient::connect(&sock).await;
    new_writer.handshake().await;
    let snap = attach_rw(&mut new_writer, &runtime_id).await;
    assert_eq!(snap.client_role, v3::WorkspaceClientRole::Writer as i32);
}

#[tokio::test]
async fn reader_survives_writer_disconnect() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let runtime_id =
        create_workspace(&mut writer, "reader-survives", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut writer, &runtime_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    attach_ro(&mut reader, &runtime_id).await;

    // Drop writer.
    drop(writer);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Reader should still be able to list workspaces.
    let workspaces = list_workspaces(&mut reader).await;
    assert_eq!(workspaces.len(), 1);
    assert!(!workspaces[0].has_write_owner);
    assert_eq!(workspaces[0].read_only_client_count, 1);
}

// ── Revision monotonicity under concurrent operations ───────────

#[tokio::test]
async fn revisions_monotonic_across_attach_detach_cycle() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c1 = TestClient::connect(&sock).await;
    c1.handshake().await;
    let runtime_id = create_workspace(&mut c1, "mono-rev", v3::WorkspacePolicy::Persistent).await;

    let mut last_rev = 0u64;

    // Attach-detach cycle with multiple clients.
    for _ in 0..3 {
        let snap = attach_rw(&mut c1, &runtime_id).await;
        assert!(snap.workspace_revision > last_rev, "revision must increase on attach");
        last_rev = snap.workspace_revision;

        detach_workspace(&mut c1, &runtime_id).await;
    }

    // Final inventory check.
    let workspaces = list_workspaces(&mut c1).await;
    assert_eq!(workspaces.len(), 1);
    assert!(workspaces[0].workspace_revision >= last_rev);
}
