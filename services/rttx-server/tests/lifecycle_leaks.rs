//! Resource-leak loop tests for runtime lifecycle.
//!
//! Repeated create/attach/detach/terminate cycles that verify the server
//! returns to a stable steady state with no leaked runtimes or panes.
//! No sleep-based timing — all assertions use polling with timeouts.

mod common;

use common::*;
use rttx_proto::v3;

#[tokio::test]
async fn create_terminate_loop_leaves_zero_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    for i in 0..10 {
        let sid =
            create_runtime(&mut client, &format!("loop-{i}"), v3::RuntimePolicy::Persistent).await;
        attach_rw(&mut client, &sid).await;
        terminate_runtime(&mut client, &sid).await;
    }

    let runtimes = list_runtimes(&mut client).await;
    assert_eq!(runtimes.len(), 0, "all terminated sessions must be cleaned up");
}

#[tokio::test]
async fn create_close_pane_loop_returns_to_zero_panes() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let sid = create_runtime(&mut client, "pane-loop", v3::RuntimePolicy::Persistent).await;
    attach_rw(&mut client, &sid).await;

    for _ in 0..10 {
        let pane_id = create_pane(&mut client, &sid).await;
        close_pane(&mut client, &sid, &pane_id).await;
    }

    let runtimes = list_runtimes(&mut client).await;
    assert_eq!(runtimes[0].pane_count, 0, "all closed panes must be cleaned up");
}

#[tokio::test]
async fn attach_detach_loop_persistent_session_survives() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let sid = create_runtime(&mut client, "detach-loop", v3::RuntimePolicy::Persistent).await;

    for _ in 0..10 {
        attach_rw(&mut client, &sid).await;
        detach_runtime(&mut client, &sid).await;
    }

    let runtimes = list_runtimes(&mut client).await;
    assert_eq!(runtimes.len(), 1, "persistent session must survive detach loops");
    assert_eq!(runtimes[0].read_only_client_count, 0);
    assert!(!runtimes[0].has_write_owner);
}

#[tokio::test]
async fn ephemeral_create_detach_loop_leaves_zero_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    for i in 0..10 {
        let sid =
            create_runtime(&mut client, &format!("eph-{i}"), v3::RuntimePolicy::Ephemeral).await;
        attach_rw(&mut client, &sid).await;
        detach_runtime(&mut client, &sid).await;
    }

    let runtimes = list_runtimes(&mut client).await;
    assert_eq!(runtimes.len(), 0, "ephemeral sessions must terminate on last detach");
}

#[tokio::test]
async fn full_lifecycle_loop_returns_to_clean_state() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    for i in 0..5 {
        let sid =
            create_runtime(&mut client, &format!("full-{i}"), v3::RuntimePolicy::Persistent).await;
        attach_rw(&mut client, &sid).await;
        let p1 = create_pane(&mut client, &sid).await;
        let p2 = create_pane(&mut client, &sid).await;
        close_pane(&mut client, &sid, &p1).await;
        close_pane(&mut client, &sid, &p2).await;
        terminate_runtime(&mut client, &sid).await;
    }

    let runtimes = list_runtimes(&mut client).await;
    assert_eq!(runtimes.len(), 0, "full lifecycle loop must leave zero sessions");
}

#[tokio::test]
async fn reconnect_loop_does_not_leak_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;
    let sid = create_runtime(&mut c, "reconnect-loop", v3::RuntimePolicy::Persistent).await;
    attach_rw(&mut c, &sid).await;
    drop(c);

    for _ in 0..10 {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        let runtimes = list_runtimes(&mut c).await;
        assert_eq!(runtimes.len(), 1, "reconnect must not create duplicate sessions");
        attach_rw(&mut c, &sid).await;
        drop(c);
    }

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;
    let runtimes = list_runtimes(&mut c).await;
    assert_eq!(runtimes.len(), 1);
    assert_eq!(runtimes[0].read_only_client_count, 0);
}
