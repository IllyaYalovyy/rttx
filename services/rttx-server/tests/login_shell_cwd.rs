//! Regression test: shells spawned by the daemon are login shells so that
//! `/etc/profile.d/vte-2.91.sh` is sourced and OSC 7 CWD reporting works.

mod common;

use common::{TestClient, send_input, start_test_server};
use rttx_proto::v3;
use std::time::Duration;

#[tokio::test]
async fn spawned_shell_is_login_shell_and_reports_cwd_via_osc7() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let runtime_id =
        common::create_runtime(&mut client, "login-shell-test", v3::RuntimePolicy::Persistent)
            .await;

    let create_pane = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
            runtime_id: runtime_id.clone(),
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
            no_persist: None,
        })),
    };
    client.send(&create_pane).await;
    let pane_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    let attach = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
            runtime_id: runtime_id.clone(),
            attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::RuntimeSnapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }

    // cd to /tmp and emit OSC 7 manually (in case CI lacks vte script).
    // Hold with `cat` to prevent prompt from overwriting.
    send_input(
        &mut client,
        &runtime_id,
        &pane_id,
        b"cd /tmp && printf '\\033]7;file://localhost/tmp\\033\\\\'; cat\n",
    )
    .await;

    let msgs = client.drain(Duration::from_secs(5)).await;
    let saw_tmp_cwd = msgs.iter().any(|m| {
        matches!(
            &m.payload,
            Some(v3::server_envelope::Payload::CwdChanged(c)) if c.cwd == "/tmp"
        )
    });

    assert!(saw_tmp_cwd, "login shell should emit OSC 7 after cd, triggering CwdChanged with /tmp");
}

/// End-to-end behavioral proof that the daemon spawns the default shell as a
/// login shell. A `no_persist` pane uses the engine's default shell command,
/// which is the path that applies the login `argv[0]` (leading `-`). We ask
/// the shell to print its own `argv[0]` (`$0`); a login shell reports a
/// leading `-` (e.g. `-bash`). This observes the user-visible effect through
/// the wire protocol instead of racing `/proc` post-exec state.
#[tokio::test]
async fn spawned_shell_reports_login_argv0() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let runtime_id =
        common::create_runtime(&mut client, "login-argv0-test", v3::RuntimePolicy::Persistent)
            .await;

    let create_pane = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
            runtime_id: runtime_id.clone(),
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
            no_persist: Some(true),
        })),
    };
    client.send(&create_pane).await;
    let pane_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    let attach = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
            runtime_id: runtime_id.clone(),
            attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::RuntimeSnapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }

    // `$0` is the shell's argv[0]; a login shell reports a leading '-'. The
    // marker substring `ARGV0=-` cannot appear in the echoed command line
    // (which contains the literal `%s`), so it only matches printf's output.
    send_input(&mut client, &runtime_id, &pane_id, b"printf 'ARGV0=%s\\n' \"$0\"; cat\n").await;

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    let mut saw_login_argv0 = false;
    while std::time::Instant::now() < deadline {
        for msg in client.drain(Duration::from_millis(200)).await {
            if let Some(v3::server_envelope::Payload::OutputDelta(delta)) = msg.payload {
                output.extend_from_slice(&delta.data);
            }
        }
        if String::from_utf8_lossy(&output).contains("ARGV0=-") {
            saw_login_argv0 = true;
            break;
        }
    }

    assert!(
        saw_login_argv0,
        "default shell must run as a login shell (argv[0] starts with '-'); captured output: {}",
        String::from_utf8_lossy(&output)
    );
}
