//! Scale and scrollback stress tests.
//!
//! Exercises larger runtime inventories, multiple panes per runtime,
//! and scrollback volume under attach and restart. Sized to stay
//! reliable in CI (~10s) while catching scale regressions.

mod common;

use common::*;
use rttx_proto::proto;
use std::time::Duration;

// ── Helpers ─────────────────────────────────────────────────────

// ── Many runtimes in inventory ──────────────────────────────────

#[tokio::test]
async fn ten_runtimes_listed_in_stable_order() {
    let tmp = tempfile::tempdir().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    for i in 0..10 {
        create_runtime(&mut client, &format!("session-{i}"), proto::RuntimePolicy::Persistent)
            .await;
    }

    let runtimes = list_runtimes(&mut client).await;
    assert_eq!(runtimes.len(), 10);

    // Inventory must be sorted by session ID (server contract).
    let listed_ids: Vec<&[u8]> = runtimes.iter().map(|s| s.id.as_slice()).collect();
    let mut sorted_ids = listed_ids.clone();
    sorted_ids.sort();
    assert_eq!(listed_ids, sorted_ids, "inventory must be sorted by session ID");

    // List again — order must be stable.
    let sessions2 = list_runtimes(&mut client).await;
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

    let runtime_id =
        create_runtime(&mut client, "multi-pane", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut client, &runtime_id).await;

    let mut pane_ids = Vec::new();
    for _ in 0..5 {
        pane_ids.push(create_pane(&mut client, &runtime_id).await);
    }

    let runtimes = list_runtimes(&mut client).await;
    assert_eq!(runtimes[0].pane_count, 5);

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

    let runtime_id =
        create_runtime(&mut client, "scrollback", proto::RuntimePolicy::Persistent).await;
    attach_rw(&mut client, &runtime_id).await;
    let pane_id = create_pane(&mut client, &runtime_id).await;

    // Send a burst of input to generate scrollback.
    for i in 0..20 {
        send_input(&mut client, &runtime_id, &pane_id, format!("echo line-{i}\n").as_bytes()).await;
    }

    // Drain Deltas until we see output from the last echo command.
    let target = b"line-19";
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for PTY output");
        match client.try_recv(remaining).await {
            Some(msg) => {
                if let Some(proto::server_message::Msg::Delta(d)) = &msg.msg
                    && d.data.windows(target.len()).any(|w| w == target)
                {
                    break;
                }
            }
            None => panic!("timed out waiting for PTY output"),
        }
    }

    // Detach.
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::DetachRuntime(proto::DetachRuntime {
                runtime_id: runtime_id.clone(),
            })),
        })
        .await;
    client.drain(Duration::from_millis(500)).await;

    // Reattach — snapshot should contain scrollback.
    let snap = attach_rw(&mut client, &runtime_id).await;
    assert!(!snap.panes.is_empty());

    let total_bytes: usize = snap.panes.iter().map(|p| p.scrollback.len()).sum();
    assert!(total_bytes > 0, "reattach snapshot must contain scrollback data");
}

// ── Large scrollback survives restart ───────────────────────────

#[tokio::test]
async fn scrollback_survives_restart() {
    let tmp = tempfile::tempdir().unwrap();

    let runtime_id;
    let pane_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        runtime_id =
            create_runtime(&mut client, "restart-scroll", proto::RuntimePolicy::Persistent).await;
        attach_rw(&mut client, &runtime_id).await;
        pane_id = create_pane(&mut client, &runtime_id).await;

        for i in 0..20 {
            send_input(
                &mut client,
                &runtime_id,
                &pane_id,
                format!("echo restart-line-{i}\n").as_bytes(),
            )
            .await;
        }

        // Wait for serialization + scrollback flush.
        // The serialization loop ticks every 1s. Wait for the v2 runtime
        // file to contain our session data.
        common::wait_for_state_containing(tmp.path(), "restart-scroll", Duration::from_secs(10))
            .await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Restart and reattach.
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let runtimes = list_runtimes(&mut client).await;
    assert_eq!(runtimes.len(), 1);
    assert!(runtimes[0].reconstructed);

    let snap = attach_rw(&mut client, &runtime_id).await;
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
        create_runtime(&mut client, &format!("load-{i}"), proto::RuntimePolicy::Persistent).await;
    }

    // List 10 times — count and order must be stable.
    let baseline = list_runtimes(&mut client).await;
    assert_eq!(baseline.len(), 5);

    for round in 0..10 {
        let runtimes = list_runtimes(&mut client).await;
        assert_eq!(runtimes.len(), 5, "round {round}: session count changed");
        let ids: Vec<&[u8]> = runtimes.iter().map(|s| s.id.as_slice()).collect();
        let baseline_ids: Vec<&[u8]> = baseline.iter().map(|s| s.id.as_slice()).collect();
        assert_eq!(ids, baseline_ids, "round {round}: inventory order changed");
    }
}
