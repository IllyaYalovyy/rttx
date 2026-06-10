//! Integration tests for shell history crash survival through the full server.
//!
//! These exercise the default `$SHELL` end-to-end: a command run in a
//! persistent pane must reach the per-pane `HISTFILE` on disk *during* normal
//! operation (incremental flush), so it survives a hard crash with no clean
//! shutdown. Shell-specific generation logic is covered by the `shell_init`
//! unit tests and the per-shell `shell_history` integration tests.

mod common;

use common::*;
use rttx_proto::v3;
use std::time::Duration;

/// After a hard crash (server kill + restart), shell history from a persistent
/// pane must survive because the shell flushed it to disk incrementally —
/// without the test or the user's rc explicitly arranging it.
#[tokio::test]
async fn history_survives_hard_restart() {
    let tmp = tempfile::TempDir::new().unwrap();

    let runtime_id;
    let pane_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        runtime_id = create_runtime(&mut client, "crash-hist", v3::RuntimePolicy::Persistent).await;
        pane_id = create_pane(&mut client, &runtime_id).await;
        attach_rw(&mut client, &runtime_id).await;

        // Run a unique command, then trigger the next prompt so the shell's
        // incremental flush writes it to disk. No manual PROMPT_COMMAND setup:
        // the generated rc handles it (RFC-031 §7).
        send_input(&mut client, &runtime_id, &pane_id, b"echo UNIQUE_MARKER_12345\n").await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            if let Some(msg) = client.try_recv(Duration::from_millis(200)).await
                && let Some(v3::server_envelope::Payload::OutputDelta(d)) = msg.payload
                && String::from_utf8_lossy(&d.data).contains("UNIQUE_MARKER_12345")
            {
                break;
            }
        }
        send_input(&mut client, &runtime_id, &pane_id, b"true\n").await;

        // Poll the on-disk HISTFILE until the incremental flush lands. Polling
        // instead of a fixed sleep keeps the test deterministic under parallel
        // load.
        let state_dir = tmp.path().join("state/rttx/daemon");
        let pane_uuid = rttx_proto::bytes_to_uuid(&pane_id).unwrap();
        let runtime_uuid = rttx_proto::bytes_to_uuid(&runtime_id).unwrap();
        let hist_path =
            rttx_server::state::layout::history_file(&state_dir, runtime_uuid, pane_uuid);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            if std::fs::read_to_string(&hist_path)
                .unwrap_or_default()
                .contains("UNIQUE_MARKER_12345")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Simulate a hard crash: abort the server with no clean shutdown.
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let state_dir = tmp.path().join("state/rttx/daemon");
    let pane_uuid = rttx_proto::bytes_to_uuid(&pane_id).unwrap();
    let runtime_uuid = rttx_proto::bytes_to_uuid(&runtime_id).unwrap();
    let hist_path = rttx_server::state::layout::history_file(&state_dir, runtime_uuid, pane_uuid);

    let hist_content = std::fs::read_to_string(&hist_path).unwrap_or_default();
    assert!(
        hist_content.contains("UNIQUE_MARKER_12345"),
        "history file must contain the command after crash, path={}, content={hist_content}",
        hist_path.display()
    );
}

/// Ephemeral (no-persist) panes must flush to `/dev/null`, never to a per-pane
/// history file — flushing disposable panes would pollute durable state.
#[tokio::test]
async fn ephemeral_pane_does_not_write_persistent_history() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let runtime_id =
        create_runtime(&mut client, "ephemeral-hist", v3::RuntimePolicy::Persistent).await;

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
                runtime_id: runtime_id.clone(),
                cwd: None,
                dark_background: None,
                cols: 80,
                rows: 24,
                no_persist: Some(true),
            })),
        })
        .await;
    let pane_id = loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::PaneCreated(pc)) => break pc.pane_id,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected PaneCreated, got {other:?}"),
        }
    };
    attach_rw(&mut client, &runtime_id).await;

    // The shell reports its HISTFILE; for an ephemeral pane it must be
    // /dev/null, not a path under the runtime's history dir.
    send_input(&mut client, &runtime_id, &pane_id, b"echo RTTX_HF=$HISTFILE\n").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    while tokio::time::Instant::now() < deadline {
        if let Some(msg) = client.try_recv(Duration::from_millis(200)).await
            && let Some(v3::server_envelope::Payload::OutputDelta(d)) = msg.payload
        {
            output.extend_from_slice(&d.data);
            // Match the resolved value, not the echoed command (which contains
            // the literal "$HISTFILE").
            if String::from_utf8_lossy(&output).contains("/dev/null") {
                break;
            }
        }
    }

    let text = String::from_utf8_lossy(&output);
    assert!(
        text.contains("RTTX_HF=/dev/null"),
        "ephemeral HISTFILE must be /dev/null, got: {text}"
    );

    let state_dir = tmp.path().join("state/rttx/daemon");
    let pane_uuid = rttx_proto::bytes_to_uuid(&pane_id).unwrap();
    let runtime_uuid = rttx_proto::bytes_to_uuid(&runtime_id).unwrap();
    let hist_path = rttx_server::state::layout::history_file(&state_dir, runtime_uuid, pane_uuid);
    assert!(
        !hist_path.exists(),
        "ephemeral pane must not create a persistent history file at {}",
        hist_path.display()
    );
}
