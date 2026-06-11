//! Resource-leak loop tests for workspace lifecycle.
//!
//! Repeated create/attach/detach/terminate cycles that verify the server
//! returns to a stable steady state with no leaked workspaces or panes.
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
            create_workspace(&mut client, &format!("loop-{i}"), v3::WorkspacePolicy::Persistent)
                .await;
        attach_rw(&mut client, &sid).await;
        terminate_workspace(&mut client, &sid).await;
    }

    let workspaces = list_workspaces(&mut client).await;
    assert_eq!(workspaces.len(), 0, "all terminated sessions must be cleaned up");
}

#[tokio::test]
async fn create_close_pane_loop_returns_to_zero_panes() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let sid = create_workspace(&mut client, "pane-loop", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut client, &sid).await;

    for _ in 0..10 {
        let pane_id = create_pane(&mut client, &sid).await;
        close_pane(&mut client, &sid, &pane_id).await;
    }

    let workspaces = list_workspaces(&mut client).await;
    assert_eq!(workspaces[0].pane_count, 0, "all closed panes must be cleaned up");
}

#[tokio::test]
async fn attach_detach_loop_persistent_session_survives() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let sid = create_workspace(&mut client, "detach-loop", v3::WorkspacePolicy::Persistent).await;

    for _ in 0..10 {
        attach_rw(&mut client, &sid).await;
        detach_workspace(&mut client, &sid).await;
    }

    let workspaces = list_workspaces(&mut client).await;
    assert_eq!(workspaces.len(), 1, "persistent session must survive detach loops");
    assert_eq!(workspaces[0].read_only_client_count, 0);
    assert!(!workspaces[0].has_write_owner);
}

#[tokio::test]
async fn ephemeral_create_detach_loop_leaves_zero_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    for i in 0..10 {
        let sid =
            create_workspace(&mut client, &format!("eph-{i}"), v3::WorkspacePolicy::Ephemeral)
                .await;
        attach_rw(&mut client, &sid).await;
        detach_workspace(&mut client, &sid).await;
    }

    let workspaces = list_workspaces(&mut client).await;
    assert_eq!(workspaces.len(), 0, "ephemeral sessions must terminate on last detach");
}

#[tokio::test]
async fn full_lifecycle_loop_returns_to_clean_state() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    for i in 0..5 {
        let sid =
            create_workspace(&mut client, &format!("full-{i}"), v3::WorkspacePolicy::Persistent)
                .await;
        attach_rw(&mut client, &sid).await;
        let p1 = create_pane(&mut client, &sid).await;
        let p2 = create_pane(&mut client, &sid).await;
        close_pane(&mut client, &sid, &p1).await;
        close_pane(&mut client, &sid, &p2).await;
        terminate_workspace(&mut client, &sid).await;
    }

    let workspaces = list_workspaces(&mut client).await;
    assert_eq!(workspaces.len(), 0, "full lifecycle loop must leave zero sessions");
}

#[tokio::test]
async fn reconnect_loop_does_not_leak_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;
    let sid = create_workspace(&mut c, "reconnect-loop", v3::WorkspacePolicy::Persistent).await;
    attach_rw(&mut c, &sid).await;
    drop(c);

    for _ in 0..10 {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        let workspaces = list_workspaces(&mut c).await;
        assert_eq!(workspaces.len(), 1, "reconnect must not create duplicate sessions");
        attach_rw(&mut c, &sid).await;
        drop(c);
    }

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;
    let workspaces = list_workspaces(&mut c).await;
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].read_only_client_count, 0);
}
