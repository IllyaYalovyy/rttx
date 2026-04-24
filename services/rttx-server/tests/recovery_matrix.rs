//! Restart and recovery behavior matrix.
//!
//! Explicit matrix covering: runtime policy × disconnect mode × client role.
//! Each test documents the expected outcome for one cell of the matrix.
//!
//! | Policy     | Disconnect Mode    | Expected After Recovery              |
//! |------------|--------------------|--------------------------------------|
//! | Persistent | Transport drop     | Session survives, reattachable       |
//! | Persistent | Daemon restart     | Session reconstructed, panes rebuilt  |
//! | Persistent | Explicit detach    | Session survives, no attached clients |
//! | Persistent | Explicit terminate | Session removed                      |
//! | Ephemeral  | Transport drop     | Session survives until restart        |
//! | Ephemeral  | Daemon restart     | Session NOT restored                 |
//! | Ephemeral  | Explicit detach    | Session terminated immediately        |

mod common;

use common::*;
use rttx_proto::proto;
use std::time::Duration;

// ── Helpers ─────────────────────────────────────────────────────

// ── Persistent × Transport disconnect ───────────────────────────

#[tokio::test]
async fn persistent_transport_drop_session_survives_and_reattaches() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let runtime_id = {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        let sid = create_runtime(&mut c, "p-drop", proto::RuntimePolicy::Persistent).await;
        attach_rw(&mut c, &sid).await;
        create_pane(&mut c, &sid).await;
        sid
        // c dropped here — transport disconnect
    };

    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut c2 = TestClient::connect(&sock).await;
    c2.handshake().await;
    let runtimes = list_runtimes(&mut c2).await;
    assert_eq!(runtimes.len(), 1);
    assert_eq!(runtimes[0].id, runtime_id);
    assert_eq!(runtimes[0].pane_count, 1);
    assert_eq!(runtimes[0].attached_client_count, 0);
    assert!(!runtimes[0].has_write_owner);

    let snap = attach_rw(&mut c2, &runtime_id).await;
    assert_eq!(snap.current_client_role, proto::RuntimeClientRole::Writer as i32);
    assert!(!snap.panes.is_empty());
}

// ── Persistent × Daemon restart ─────────────────────────────────

#[tokio::test]
async fn persistent_daemon_restart_reconstructs_session_and_panes() {
    let tmp = tempfile::tempdir().unwrap();

    let runtime_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        runtime_id = create_runtime(&mut c, "p-restart", proto::RuntimePolicy::Persistent).await;
        attach_rw(&mut c, &runtime_id).await;
        create_pane(&mut c, &runtime_id).await;

        // Wait for serialization tick.
        wait_for_state_containing(tmp.path(), "p-restart", Duration::from_secs(10))
            .await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Restart.
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    let runtimes = list_runtimes(&mut c).await;
    assert_eq!(runtimes.len(), 1);
    assert_eq!(runtimes[0].id, runtime_id);
    assert!(runtimes[0].reconstructed);
    assert_eq!(runtimes[0].pane_count, 1);
    assert_eq!(runtimes[0].attached_client_count, 0);

    let snap = attach_rw(&mut c, &runtime_id).await;
    assert_eq!(snap.panes.len(), 1);
    // reconstructed flag is on PaneInfo (inventory), not PaneSnapshot.
    assert!(snap.revision > 0);
}

// ── Persistent × Explicit detach ────────────────────────────────

#[tokio::test]
async fn persistent_explicit_detach_runtime_survives_unattached() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;
    let runtime_id = create_runtime(&mut c, "p-detach", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut c, &runtime_id).await;
    create_pane(&mut c, &runtime_id).await;

    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::DetachRuntime(proto::DetachRuntime {
            runtime_id: runtime_id.clone(),
        })),
    })
    .await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for RuntimeDetached");
        match c.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::RuntimeDetached(d)) => {
                assert_eq!(d.runtime_id, runtime_id);
                break;
            }
            Some(
                proto::server_message::Msg::Delta(_) | proto::server_message::Msg::PaneExited(_),
            ) => {}
            other => panic!("expected RuntimeDetached, got {other:?}"),
        }
    }

    let runtimes = list_runtimes(&mut c).await;
    assert_eq!(runtimes.len(), 1);
    assert_eq!(runtimes[0].attached_client_count, 0);
    assert!(!runtimes[0].has_write_owner);
    assert_eq!(runtimes[0].pane_count, 1);
}

// ── Persistent × Explicit terminate ─────────────────────────────

#[tokio::test]
async fn persistent_explicit_terminate_removes_session() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;
    let runtime_id = create_runtime(&mut c, "p-term", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut c, &runtime_id).await;

    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::TerminateRuntime(proto::TerminateRuntime {
            runtime_id: runtime_id.clone(),
        })),
    })
    .await;
    match c.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::RuntimeTerminated(t)) => {
            assert_eq!(t.runtime_id, runtime_id);
            assert_eq!(t.reason, proto::RuntimeTerminationReason::Explicit as i32);
        }
        other => panic!("expected RuntimeTerminated, got {other:?}"),
    }

    let runtimes = list_runtimes(&mut c).await;
    assert!(runtimes.is_empty());
}

// ── Ephemeral × Transport disconnect ────────────────────────────

#[tokio::test]
async fn ephemeral_transport_drop_session_survives_until_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let runtime_id = {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        let sid = create_runtime(&mut c, "e-drop", proto::RuntimePolicy::Ephemeral).await;
        attach_rw(&mut c, &sid).await;
        sid
    };

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Session still exists after transport drop (not explicit detach).
    let mut c2 = TestClient::connect(&sock).await;
    c2.handshake().await;
    let runtimes = list_runtimes(&mut c2).await;
    assert_eq!(runtimes.len(), 1);
    assert_eq!(runtimes[0].id, runtime_id);
    assert_eq!(
        proto::RuntimePolicy::try_from(runtimes[0].policy).unwrap(),
        proto::RuntimePolicy::Ephemeral
    );
}

// ── Ephemeral × Daemon restart ──────────────────────────────────

#[tokio::test]
async fn ephemeral_daemon_restart_does_not_restore_session() {
    let tmp = tempfile::tempdir().unwrap();

    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        let sid = create_runtime(&mut c, "e-restart", proto::RuntimePolicy::Ephemeral).await;
        attach_rw(&mut c, &sid).await;
        create_pane(&mut c, &sid).await;

        // Create a persistent runtime so we can wait for the serialization
        // loop to have run at least once (ephemeral runtimes are not persisted).
        let _ = create_runtime(&mut c, "e-restart-anchor", proto::RuntimePolicy::Persistent).await;
        wait_for_state_containing(
            tmp.path(),
            "e-restart-anchor",
            Duration::from_secs(10),
        )
        .await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    let runtimes = list_runtimes(&mut c).await;
    assert_eq!(
        runtimes.len(),
        1,
        "only the persistent anchor should survive restart, not the ephemeral runtime"
    );
    assert_eq!(runtimes[0].name, "e-restart-anchor");
}

// ── Ephemeral × Explicit detach ─────────────────────────────────

#[tokio::test]
async fn ephemeral_explicit_detach_terminates_immediately() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;
    let runtime_id = create_runtime(&mut c, "e-detach", proto::RuntimePolicy::Ephemeral).await;
    attach_rw(&mut c, &runtime_id).await;

    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::DetachRuntime(proto::DetachRuntime {
            runtime_id: runtime_id.clone(),
        })),
    })
    .await;
    match c.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::RuntimeTerminated(t)) => {
            assert_eq!(t.runtime_id, runtime_id);
            assert_eq!(t.reason, proto::RuntimeTerminationReason::EphemeralLastDetach as i32);
        }
        other => panic!("expected RuntimeTerminated, got {other:?}"),
    }

    let runtimes = list_runtimes(&mut c).await;
    assert!(runtimes.is_empty());
}

// ── Persistent × Restart with read-only client role ─────────────

#[tokio::test]
async fn persistent_restart_reader_reattaches_after_reconstruction() {
    let tmp = tempfile::tempdir().unwrap();

    let runtime_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut writer = TestClient::connect(&sock).await;
        writer.handshake().await;
        runtime_id =
            create_runtime(&mut writer, "p-reader-restart", proto::RuntimePolicy::Persistent).await;
        attach_rw(&mut writer, &runtime_id).await;
        create_pane(&mut writer, &runtime_id).await;

        wait_for_state_containing(
            tmp.path(),
            "p-reader-restart",
            Duration::from_secs(10),
        )
        .await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut reader = TestClient::connect(&sock).await;
    reader.handshake().await;

    // Attach as read-only after reconstruction.
    reader
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
                runtime_id: runtime_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadOnly as i32,
            })),
        })
        .await;
    match reader.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::Snapshot(snap)) => {
            assert_eq!(snap.current_client_role, proto::RuntimeClientRole::Reader as i32);
            assert!(!snap.panes.is_empty());
            // reconstructed flag is on PaneInfo (inventory), not PaneSnapshot.
        }
        other => panic!("expected Snapshot, got {other:?}"),
    }

    let runtimes = list_runtimes(&mut reader).await;
    assert_eq!(runtimes[0].read_only_client_count, 1);
    assert!(!runtimes[0].has_write_owner);
}
