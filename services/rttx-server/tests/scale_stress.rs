//! Scale and scrollback stress tests.
//!
//! Exercises larger runtime inventories, multiple panes per runtime,
//! and scrollback volume under attach and restart. Sized to stay
//! reliable in CI (~10s) while catching scale regressions.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::proto;
use std::time::Duration;

// ── Helpers ─────────────────────────────────────────────────────

async fn create_session(
    client: &mut TestClient,
    name: &str,
    policy: proto::RuntimePolicy,
) -> Vec<u8> {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: name.into(),
                policy: policy as i32,
            })),
        })
        .await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::SessionCreated(sc)) => sc.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    }
}

async fn attach_rw(client: &mut TestClient, session_id: &[u8]) -> proto::Snapshot {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.to_vec(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::Snapshot(s)) => s,
        other => panic!("expected Snapshot, got {other:?}"),
    }
}

async fn create_pane(client: &mut TestClient, session_id: &[u8]) -> Vec<u8> {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                session_id: session_id.to_vec(),
            })),
        })
        .await;
    // Drain Deltas from earlier panes until we find PaneCreated.
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::PaneCreated(pc)) => return pc.pane_id,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected PaneCreated, got {other:?}"),
        }
    }
}

async fn list_sessions(client: &mut TestClient) -> Vec<proto::SessionInfo> {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
        })
        .await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::SessionList(sl)) => sl.sessions,
        other => panic!("expected SessionList, got {other:?}"),
    }
}

async fn send_input(client: &mut TestClient, session_id: &[u8], pane_id: &[u8], data: &[u8]) {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Input(proto::Input {
                session_id: session_id.to_vec(),
                pane_id: pane_id.to_vec(),
                data: data.to_vec(),
            })),
        })
        .await;
}

// ── Many runtimes in inventory ──────────────────────────────────

#[tokio::test]
async fn ten_runtimes_listed_in_stable_order() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    for i in 0..10 {
        create_session(&mut client, &format!("session-{i}"), proto::RuntimePolicy::Persistent)
            .await;
    }

    let sessions = list_sessions(&mut client).await;
    assert_eq!(sessions.len(), 10);

    // Inventory must be sorted by session ID (server contract).
    let listed_ids: Vec<&[u8]> = sessions.iter().map(|s| s.id.as_slice()).collect();
    let mut sorted_ids = listed_ids.clone();
    sorted_ids.sort();
    assert_eq!(listed_ids, sorted_ids, "inventory must be sorted by session ID");

    // List again — order must be stable.
    let sessions2 = list_sessions(&mut client).await;
    let listed_ids2: Vec<&[u8]> = sessions2.iter().map(|s| s.id.as_slice()).collect();
    assert_eq!(listed_ids, listed_ids2, "inventory order must be stable across calls");
}

// ── Many panes in a single runtime ──────────────────────────────

#[tokio::test]
async fn five_panes_in_one_runtime() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let session_id =
        create_session(&mut client, "multi-pane", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut client, &session_id).await;

    let mut pane_ids = Vec::new();
    for _ in 0..5 {
        pane_ids.push(create_pane(&mut client, &session_id).await);
    }

    let sessions = list_sessions(&mut client).await;
    assert_eq!(sessions[0].pane_count, 5);

    // All pane IDs must be unique.
    let mut sorted = pane_ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 5, "all pane IDs must be unique");
}

// ── Large scrollback before attach ──────────────────────────────

#[tokio::test]
async fn large_scrollback_survives_detach_and_reattach() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let session_id =
        create_session(&mut client, "scrollback", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut client, &session_id).await;
    let pane_id = create_pane(&mut client, &session_id).await;

    // Send a burst of input to generate scrollback.
    for i in 0..20 {
        send_input(&mut client, &session_id, &pane_id, format!("echo line-{i}\n").as_bytes()).await;
    }

    // Let PTY process and serialization tick.
    tokio::time::sleep(Duration::from_millis(3000)).await;

    // Detach.
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::DetachSession(proto::DetachSession {
                session_id: session_id.clone(),
            })),
        })
        .await;
    client.drain(Duration::from_millis(500)).await;

    // Reattach — snapshot should contain scrollback.
    let snap = attach_rw(&mut client, &session_id).await;
    assert!(!snap.panes.is_empty());

    let total_bytes: usize = snap.panes.iter().map(|p| p.scrollback.len()).sum();
    assert!(total_bytes > 0, "reattach snapshot must contain scrollback data");
}

// ── Large scrollback survives restart ───────────────────────────

#[tokio::test]
async fn scrollback_survives_restart() {
    let tmp = tempfile::tempdir().unwrap();

    let session_id;
    let pane_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        session_id =
            create_session(&mut client, "restart-scroll", proto::RuntimePolicy::Persistent).await;
        attach_rw(&mut client, &session_id).await;
        pane_id = create_pane(&mut client, &session_id).await;

        for i in 0..20 {
            send_input(
                &mut client,
                &session_id,
                &pane_id,
                format!("echo restart-line-{i}\n").as_bytes(),
            )
            .await;
        }

        // Wait for serialization + scrollback flush.
        tokio::time::sleep(Duration::from_millis(3000)).await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Restart and reattach.
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let sessions = list_sessions(&mut client).await;
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].reconstructed);

    let snap = attach_rw(&mut client, &session_id).await;
    let total_bytes: usize = snap.panes.iter().map(|p| p.scrollback.len()).sum();
    assert!(total_bytes > 0, "scrollback must survive restart");
}

// ── Repeated list under load ────────────────────────────────────

#[tokio::test]
async fn repeated_list_under_load_is_consistent() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    for i in 0..5 {
        create_session(&mut client, &format!("load-{i}"), proto::RuntimePolicy::Persistent).await;
    }

    // List 10 times — count and order must be stable.
    let baseline = list_sessions(&mut client).await;
    assert_eq!(baseline.len(), 5);

    for round in 0..10 {
        let sessions = list_sessions(&mut client).await;
        assert_eq!(sessions.len(), 5, "round {round}: session count changed");
        let ids: Vec<&[u8]> = sessions.iter().map(|s| s.id.as_slice()).collect();
        let baseline_ids: Vec<&[u8]> = baseline.iter().map(|s| s.id.as_slice()).collect();
        assert_eq!(ids, baseline_ids, "round {round}: inventory order changed");
    }
}
