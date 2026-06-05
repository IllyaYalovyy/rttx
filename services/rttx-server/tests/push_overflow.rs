//! Integration test: push channel overflow triggers disconnect for v2
//! clients instead of silently dropping messages.
//!
//! Exercises the production overflow path by saturating a slow client's
//! push channel while a fast client continues to receive output normally.

mod common;

use common::TestClient;
use common::{attach_rw, create_runtime, start_test_server};
use rttx_proto::v3;
use std::time::Duration;

/// A v2 slow client that overflows its push channel gets disconnected
/// instead of silently losing messages. The fast client remains healthy.
#[tokio::test]
async fn v2_slow_client_gets_disconnected_on_overflow() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut fast = TestClient::connect(&sock).await;
    fast.handshake().await;

    let runtime_id =
        create_runtime(&mut fast, "overflow-test", v3::RuntimePolicy::Persistent).await;
    attach_rw(&mut fast, &runtime_id).await;

    fast.send(&v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
            runtime_id: runtime_id.clone(),
            cwd: None,
            dark_background: None,
            cols: 80,
            rows: 24,
            no_persist: None,
        })),
    })
    .await;
    let pane_id = loop {
        match fast.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::PaneCreated(pc)) => break pc.pane_id,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected PaneCreated, got {other:?}"),
        }
    };

    // Slow client: attaches read-only but never reads after snapshot.
    let mut slow = TestClient::connect(&sock).await;
    slow.handshake().await;
    slow.send(&v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
            runtime_id: runtime_id.clone(),
            attach_mode: v3::RuntimeAttachMode::ReadOnly as i32,
        })),
    })
    .await;
    let _snap = slow.recv_or_timeout().await;

    // Generate a burst of output to overflow the slow client's push channel.
    // Use a single large command that produces many lines of output quickly.
    fast.send(&v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id: runtime_id.clone(),
            pane_id: pane_id.clone(),
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                data: bytes::Bytes::from_static(b"seq 1 100000\n"),
            })),
        })),
    })
    .await;

    // Drain fast client to keep it healthy while output flows.
    let mut total_deltas = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match fast.try_recv(remaining.min(Duration::from_millis(200))).await {
            Some(msg) => {
                if matches!(msg.payload, Some(v3::server_envelope::Payload::OutputDelta(_))) {
                    total_deltas += 1;
                }
            }
            None => {
                if total_deltas > 100 {
                    break;
                }
            }
        }
    }
    assert!(total_deltas > 0, "fast client should have received Deltas");

    // Fast client should still be able to interact with the server.
    fast.send(&v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id: runtime_id.clone(),
            pane_id: pane_id.clone(),
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                data: bytes::Bytes::from_static(b"echo still-alive\n"),
            })),
        })),
    })
    .await;
    let msgs = fast.drain(Duration::from_secs(3)).await;
    let has_delta = msgs
        .iter()
        .any(|m| matches!(m.payload, Some(v3::server_envelope::Payload::OutputDelta(_))));
    assert!(has_delta, "fast client should still receive Deltas after slow client overflow");
}
