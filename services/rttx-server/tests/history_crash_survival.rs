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

/// Daemon-level invariant: history the shell flushes to its per-pane HISTFILE
/// during normal operation survives a hard crash (no clean shutdown), because
/// it is on disk keyed on `PaneId`. Per-shell auto-flush wiring is covered by
/// `shell_history.rs`; here we enable `history -a` explicitly so the test is
/// deterministic regardless of the default `$SHELL` on the CI host.
#[tokio::test]
async fn history_survives_hard_restart() {
    let tmp = tempfile::TempDir::new().unwrap();

    let runtime_id;
    let pane_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&sock).await;
        client.handshake().await;

        runtime_id = create_workspace(&mut client, "crash-hist", v3::WorkspacePolicy::Persistent).await;
        pane_id = create_pane(&mut client, &runtime_id).await;
        attach_rw(&mut client, &runtime_id).await;

        // Enable incremental flush in the running shell so the test does not
        // depend on which shell-init path the daemon picked for $SHELL.
        send_input(&mut client, &runtime_id, &pane_id, b"PROMPT_COMMAND='history -a'\n").await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Run a unique command, then trigger the next prompt so the flush
        // writes it to disk.
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
        let workspace_uuid = rttx_proto::bytes_to_uuid(&runtime_id).unwrap();
        let hist_path =
            rttx_server::state::layout::history_file(&state_dir, workspace_uuid, pane_uuid);
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
    let workspace_uuid = rttx_proto::bytes_to_uuid(&runtime_id).unwrap();
    let hist_path = rttx_server::state::layout::history_file(&state_dir, workspace_uuid, pane_uuid);

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
        create_workspace(&mut client, "ephemeral-hist", v3::WorkspacePolicy::Persistent).await;

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
    // /dev/null, not a path under the workspace's history dir.
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
    let workspace_uuid = rttx_proto::bytes_to_uuid(&runtime_id).unwrap();
    let hist_path = rttx_server::state::layout::history_file(&state_dir, workspace_uuid, pane_uuid);
    assert!(
        !hist_path.exists(),
        "ephemeral pane must not create a persistent history file at {}",
        hist_path.display()
    );
}
