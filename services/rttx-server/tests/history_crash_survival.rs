//! Integration tests for shell history crash survival.
//!
//! Verifies that spawned panes have `PROMPT_COMMAND` set to flush history
//! after every command, so history survives hard crashes.

mod common;

use common::*;
use rttx_proto::v3;
use std::time::Duration;

/// Persistent panes must have `history -a` in the `PROMPT_COMMAND` env var
/// passed to the shell process at spawn time.
#[tokio::test]
async fn persistent_pane_spawns_with_history_flush_env() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let runtime_id =
        create_runtime(&mut client, "history-flush", v3::RuntimePolicy::Persistent).await;
    let pane_id = create_pane(&mut client, &runtime_id).await;
    attach_rw(&mut client, &runtime_id).await;

    // Use /proc to read the initial environment passed to the shell process.
    // This is more reliable than echoing $PROMPT_COMMAND because rc files may
    // modify it after shell startup.
    send_input(
        &mut client,
        &runtime_id,
        &pane_id,
        b"cat /proc/$$/environ | tr '\\0' '\\n' | grep --color=never PROMPT_COMMAND\n",
    )
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    while tokio::time::Instant::now() < deadline {
        if let Some(msg) = client.try_recv(Duration::from_millis(200)).await
            && let Some(v3::server_envelope::Payload::OutputDelta(d)) = msg.payload
        {
            output.extend_from_slice(&d.data);
            let text = String::from_utf8_lossy(&output);
            if text.contains("PROMPT_COMMAND=") && text.contains("history") {
                break;
            }
        }
    }

    let text = String::from_utf8_lossy(&output);
    assert!(
        text.contains("PROMPT_COMMAND=") && text.contains("history -a"),
        "spawn env must contain PROMPT_COMMAND with 'history -a', got: {text}"
    );
}

/// Ephemeral (no-persist) panes must NOT get history -a since their
/// HISTFILE is /dev/null — flushing to /dev/null is pointless overhead.
#[tokio::test]
async fn ephemeral_pane_skips_history_flush_env() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let runtime_id =
        create_runtime(&mut client, "ephemeral-hist", v3::RuntimePolicy::Persistent).await;

    // Create pane with no_persist=true.
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

    // Read PROMPT_COMMAND from /proc environ.
    send_input(
        &mut client,
        &runtime_id,
        &pane_id,
        b"cat /proc/$$/environ | tr '\\0' '\\n' | grep --color=never PROMPT_COMMAND || echo NO_PROMPT_CMD\n",
    )
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    while tokio::time::Instant::now() < deadline {
        if let Some(msg) = client.try_recv(Duration::from_millis(200)).await
            && let Some(v3::server_envelope::Payload::OutputDelta(d)) = msg.payload
        {
            output.extend_from_slice(&d.data);
            let text = String::from_utf8_lossy(&output);
            if text.contains("NO_PROMPT_CMD") || text.contains("PROMPT_COMMAND") {
                break;
            }
        }
    }

    let text = String::from_utf8_lossy(&output);
    assert!(
        !text.contains("history -a"),
        "ephemeral pane env must NOT contain 'history -a', got: {text}"
    );
}

/// After a hard crash (server kill + restart), shell history from a
/// persistent pane must survive because `history -a` flushed it to disk.
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

        // The shell inherits PROMPT_COMMAND="history -a" but user rc files
        // may overwrite it. Ensure history -a is active for this test by
        // explicitly setting PROMPT_COMMAND in the shell.
        send_input(&mut client, &runtime_id, &pane_id, b"PROMPT_COMMAND='history -a'\n").await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Run a unique command so it gets written to history.
        send_input(&mut client, &runtime_id, &pane_id, b"echo UNIQUE_MARKER_12345\n").await;

        // Wait for output to confirm command executed.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            if let Some(msg) = client.try_recv(Duration::from_millis(200)).await
                && let Some(v3::server_envelope::Payload::OutputDelta(d)) = msg.payload
            {
                let text = String::from_utf8_lossy(&d.data);
                if text.contains("UNIQUE_MARKER_12345") {
                    break;
                }
            }
        }

        // Trigger the next prompt so PROMPT_COMMAND runs history -a.
        send_input(&mut client, &runtime_id, &pane_id, b"true\n").await;
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Simulate hard crash: abort the handle.
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Verify the HISTFILE on disk contains the command.
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
