//! Multi-client ownership race integration tests.
//!
//! Exercises concurrent access patterns: competing writer attaches,
//! read-only clients during mutations, detach-vs-terminate races,
//! and writer disconnect during pane operations.

mod common;

use common::*;
use rttx_proto::proto;
use std::time::Duration;

// ── Helpers ─────────────────────────────────────────────────────

// ── Competing writer attaches ───────────────────────────────────

#[tokio::test]
async fn three_competing_writers_only_first_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c1 = TestClient::connect(&sock).await;
    c1.handshake().await;
    let runtime_id = create_runtime(&mut c1, "race", proto::RuntimePolicy::Persistent).await;
    let snap = attach_rw(&mut c1, &runtime_id).await;
    assert_eq!(snap.current_client_role, proto::RuntimeClientRole::Writer as i32);

    for i in 0..2 {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
                runtime_id: runtime_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
        match c.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::AttachBlocked(b)) => {
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
        create_runtime(&mut writer, "push-test", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut writer, &runtime_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    attach_ro(&mut reader, &runtime_id).await;

    // Writer creates a pane.
    writer
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
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
        matches!(writer_resp.msg, Some(proto::server_message::Msg::PaneCreated(_))),
        "writer should get PaneCreated"
    );

    // Reader receives Delta pushes from the new pane's PTY output.
    let reader_msgs = reader.drain(Duration::from_secs(2)).await;
    assert!(
        reader_msgs.iter().any(|m| matches!(m.msg, Some(proto::server_message::Msg::Delta(_)))),
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
        create_runtime(&mut writer, "rev-test", proto::RuntimePolicy::Persistent).await;
    let snap = attach_rw(&mut writer, &runtime_id).await;
    let base_rev = snap.revision;

    let mut r1 = TestClient::connect(&sock).await;
    r1.handshake().await;
    let s1 = attach_ro(&mut r1, &runtime_id).await;

    let mut r2 = TestClient::connect(&sock).await;
    r2.handshake().await;
    let s2 = attach_ro(&mut r2, &runtime_id).await;

    // Each reader attach bumps revision.
    assert!(s1.revision > base_rev);
    assert!(s2.revision > s1.revision);

    // Inventory should show consistent counts.
    let runtimes = list_runtimes(&mut r2).await;
    assert_eq!(runtimes[0].attached_client_count, 3);
    assert_eq!(runtimes[0].read_only_client_count, 2);
    assert!(runtimes[0].has_write_owner);
}

// ── Detach vs terminate races ───────────────────────────────────

#[tokio::test]
async fn writer_detach_then_reader_detach_leaves_clean_state() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let runtime_id =
        create_runtime(&mut writer, "detach-race", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut writer, &runtime_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    attach_ro(&mut reader, &runtime_id).await;

    // Writer detaches first.
    detach_runtime(&mut writer, &runtime_id).await;
    // Reader gets RuntimeDetached push.
    reader.drain(Duration::from_millis(200)).await;

    // Reader detaches.
    detach_runtime(&mut reader, &runtime_id).await;

    // Session should still exist (persistent policy).
    let mut checker = TestClient::connect(&sock).await;
    checker.handshake().await;
    let runtimes = list_runtimes(&mut checker).await;
    assert_eq!(runtimes.len(), 1);
    assert!(!runtimes[0].has_write_owner);
    assert_eq!(runtimes[0].attached_client_count, 0);
}

#[tokio::test]
async fn terminate_while_reader_attached_notifies_reader() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let runtime_id =
        create_runtime(&mut writer, "term-race", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut writer, &runtime_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    attach_ro(&mut reader, &runtime_id).await;

    // Writer terminates.
    writer
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::TerminateRuntime(proto::TerminateRuntime {
                runtime_id: runtime_id.clone(),
            })),
        })
        .await;

    // Both should get RuntimeTerminated.
    let w_resp = writer.recv_or_timeout().await;
    assert!(matches!(w_resp.msg, Some(proto::server_message::Msg::RuntimeTerminated(_))));

    let r_resp = reader.recv_or_timeout().await;
    assert!(matches!(r_resp.msg, Some(proto::server_message::Msg::RuntimeTerminated(_))));

    // Session gone.
    let mut checker = TestClient::connect(&sock).await;
    checker.handshake().await;
    let runtimes = list_runtimes(&mut checker).await;
    assert!(runtimes.is_empty());
}

// ── Writer disconnect during pane operations ────────────────────

#[tokio::test]
async fn writer_disconnect_frees_ownership_for_new_writer() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let runtime_id =
        create_runtime(&mut writer, "disconnect", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut writer, &runtime_id).await;

    // Drop the writer (simulates disconnect).
    drop(writer);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // New client should be able to attach as writer.
    let mut new_writer = TestClient::connect(&sock).await;
    new_writer.handshake().await;
    let snap = attach_rw(&mut new_writer, &runtime_id).await;
    assert_eq!(snap.current_client_role, proto::RuntimeClientRole::Writer as i32);
}

#[tokio::test]
async fn reader_survives_writer_disconnect() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut writer = TestClient::connect(&sock).await;
    writer.handshake().await;
    let runtime_id =
        create_runtime(&mut writer, "reader-survives", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut writer, &runtime_id).await;

    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;
    attach_ro(&mut reader, &runtime_id).await;

    // Drop writer.
    drop(writer);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Reader should still be able to list runtimes.
    let runtimes = list_runtimes(&mut reader).await;
    assert_eq!(runtimes.len(), 1);
    assert!(!runtimes[0].has_write_owner);
    assert_eq!(runtimes[0].read_only_client_count, 1);
}

// ── Revision monotonicity under concurrent operations ───────────

#[tokio::test]
async fn revisions_monotonic_across_attach_detach_cycle() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c1 = TestClient::connect(&sock).await;
    c1.handshake().await;
    let runtime_id = create_runtime(&mut c1, "mono-rev", proto::RuntimePolicy::Persistent).await;

    let mut last_rev = 0u64;

    // Attach-detach cycle with multiple clients.
    for _ in 0..3 {
        let snap = attach_rw(&mut c1, &runtime_id).await;
        assert!(snap.revision > last_rev, "revision must increase on attach");
        last_rev = snap.revision;

        detach_runtime(&mut c1, &runtime_id).await;
    }

    // Final inventory check.
    let runtimes = list_runtimes(&mut c1).await;
    assert_eq!(runtimes.len(), 1);
    assert!(runtimes[0].revision >= last_rev);
}
