//! VTE parity harness — daemon integration layer.
//!
//! Verifies that managed terminal input bytes, when sent through the
//! daemon protocol to a real PTY, produce the expected behavior.
//! Covers: printable input, control keys, navigation, bracketed paste,
//! and stateful terminal mode preservation across reconnect/reattach.
//!
//! Required regression layer for terminal input and mouse fixes (#464).

mod common;

use common::TestClient;
use rttx_proto::v3;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::{Child, Command};

const PROMPT: &str = "PROMPT> ";

async fn start_binary_server(tmp: &tempfile::TempDir) -> (PathBuf, Child) {
    let runtime_dir = tmp.path().join("runtime");
    let cache_dir = tmp.path().join("cache");
    let config_dir = tmp.path().join("config");
    let home_dir = tmp.path().join("home");
    let shell_path = tmp.path().join("test-shell");

    std::fs::create_dir_all(&runtime_dir).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&home_dir).unwrap();

    std::fs::write(
        &shell_path,
        format!("#!/bin/sh\nexport PS1='{PROMPT}'\nexec /bin/bash --noprofile --norc -i\n"),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&shell_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&shell_path, perms).unwrap();

    let socket_path = runtime_dir.join("rttx-server").join("v1").join("rttx-server.sock");

    let child = Command::new(env!("CARGO_BIN_EXE_rttx-server"))
        .arg("start")
        .arg("--foreground")
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("RTTX_DEV_MODE", "")
        .env("XDG_CACHE_HOME", &cache_dir)
        .env("XDG_CONFIG_HOME", &config_dir)
        .env("XDG_STATE_HOME", tmp.path())
        .env("HOME", &home_dir)
        .env("SHELL", &shell_path)
        .env("TERM", "xterm-256color")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to spawn rttx-server binary");

    wait_for_socket(&socket_path).await;
    (socket_path, child)
}

async fn wait_for_socket(socket_path: &Path) {
    for _ in 0..100 {
        if socket_path.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("server socket did not appear at {}", socket_path.display());
}

async fn setup_attached_pane(client: &mut TestClient) -> (Vec<u8>, Vec<u8>) {
    client.handshake().await;

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
                name: "parity-test".into(),
                policy: v3::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
                runtime_id: runtime_id.clone(),
                cwd: None,
                dark_background: None,
                cols: 0,
                rows: 0,
                no_persist: None,
            })),
        })
        .await;
    let pane_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
                runtime_id: runtime_id.clone(),
                attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::RuntimeSnapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }

    (runtime_id, pane_id)
}

async fn wait_for_prompt(client: &mut TestClient) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut output = Vec::new();
    while tokio::time::Instant::now() < deadline {
        if let Some(msg) = client.try_recv(Duration::from_millis(200)).await
            && let Some(v3::server_envelope::Payload::OutputDelta(delta)) = msg.payload
        {
            output.extend(delta.data);
            if String::from_utf8_lossy(&output).contains(PROMPT) {
                return;
            }
        }
    }
    panic!("shell prompt did not arrive within 30 seconds");
}

async fn send_input(client: &mut TestClient, runtime_id: &[u8], pane_id: &[u8], data: &[u8]) {
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
                runtime_id: runtime_id.to_vec(),
                pane_id: pane_id.to_vec(),
                kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                    data: bytes::Bytes::copy_from_slice(data),
                })),
            })),
        })
        .await;
}

async fn collect_output(client: &mut TestClient, window: Duration) -> String {
    let msgs = client.drain(window).await;
    let bytes: Vec<u8> = msgs
        .iter()
        .filter_map(|m| match &m.payload {
            Some(v3::server_envelope::Payload::OutputDelta(d)) => Some(d.data.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    String::from_utf8_lossy(&bytes).to_string()
}

async fn shutdown_server(client: &mut TestClient, server_child: &mut Child) {
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::Shutdown(v3::Shutdown {})),
        })
        .await;
    let status = tokio::time::timeout(Duration::from_secs(10), server_child.wait())
        .await
        .expect("timed out waiting for rttx-server to stop")
        .expect("failed to wait for rttx-server child");
    assert!(status.success(), "rttx-server exited unsuccessfully: {status}");
}

async fn reattach_snapshot_text(
    client: &mut TestClient,
    runtime_id: &[u8],
    pane_id: &[u8],
) -> String {
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::DetachRuntime(v3::DetachRuntime {
                runtime_id: runtime_id.to_vec(),
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::RuntimeDetached(_)) => break,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected RuntimeDetached, got {other:?}"),
        }
    }

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
                runtime_id: runtime_id.to_vec(),
                attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    let snapshot = loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::RuntimeSnapshot(s)) => break s,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }
    };
    let scrollback = snapshot
        .panes
        .iter()
        .find(|p| p.pane_id == pane_id)
        .expect("pane missing from snapshot")
        .scrollback_tail
        .clone();
    normalize(&scrollback)
}

fn normalize(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .replace("\x1b[?2004h", "")
        .replace("\x1b[?2004l", "")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

// ── Printable input through daemon ──────────────────────────────

#[tokio::test]
async fn printable_ascii_echoes_through_daemon_pty() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, mut server) = start_binary_server(&tmp).await;

    let mut client = TestClient::connect(&sock).await;
    let (sid, pid) = setup_attached_pane(&mut client).await;
    wait_for_prompt(&mut client).await;

    send_input(&mut client, &sid, &pid, b"echo hello123\r").await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let output = collect_output(&mut client, Duration::from_secs(2)).await;
    assert!(output.contains("hello123"), "printable ASCII must echo: {output:?}");

    shutdown_server(&mut client, &mut server).await;
}

// ── Control keys through daemon ─────────────────────────────────

#[tokio::test]
async fn ctrl_c_interrupts_running_command() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, mut server) = start_binary_server(&tmp).await;

    let mut client = TestClient::connect(&sock).await;
    let (sid, pid) = setup_attached_pane(&mut client).await;
    wait_for_prompt(&mut client).await;

    // Start a long sleep, then interrupt with Ctrl+C (0x03).
    send_input(&mut client, &sid, &pid, b"sleep 999\r").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    send_input(&mut client, &sid, &pid, &[0x03]).await;

    // Should get back to prompt.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut output = String::new();
    while tokio::time::Instant::now() < deadline {
        output.push_str(&collect_output(&mut client, Duration::from_millis(200)).await);
        if output.contains(PROMPT) {
            break;
        }
    }
    assert!(output.contains(PROMPT), "Ctrl+C must return to prompt: {output:?}");

    shutdown_server(&mut client, &mut server).await;
}

// ── Navigation keys through daemon ──────────────────────────────

#[tokio::test]
async fn arrow_keys_navigate_shell_history() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, mut server) = start_binary_server(&tmp).await;

    let mut client = TestClient::connect(&sock).await;
    let (sid, pid) = setup_attached_pane(&mut client).await;
    wait_for_prompt(&mut client).await;

    // Run a command, then use Up arrow to recall it.
    send_input(&mut client, &sid, &pid, b"echo ARROW_TEST\r").await;
    wait_for_prompt(&mut client).await;

    // Up arrow = \x1b[A, then Enter to re-execute.
    send_input(&mut client, &sid, &pid, b"\x1b[A\r").await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let output = collect_output(&mut client, Duration::from_secs(2)).await;
    assert!(output.contains("ARROW_TEST"), "Up arrow must recall previous command: {output:?}");

    shutdown_server(&mut client, &mut server).await;
}

// ── Bracketed paste through daemon ──────────────────────────────

#[tokio::test]
async fn bracketed_paste_wraps_content_correctly() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, mut server) = start_binary_server(&tmp).await;

    let mut client = TestClient::connect(&sock).await;
    let (sid, pid) = setup_attached_pane(&mut client).await;
    wait_for_prompt(&mut client).await;

    // Bash enables bracketed paste by default. Send paste-wrapped content.
    // Bracketed paste: ESC[200~ <content> ESC[201~
    let paste_content = b"echo PASTED_OK";
    let mut paste_bytes = b"\x1b[200~".to_vec();
    paste_bytes.extend_from_slice(paste_content);
    paste_bytes.extend_from_slice(b"\x1b[201~");
    paste_bytes.push(b'\r');

    send_input(&mut client, &sid, &pid, &paste_bytes).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let output = collect_output(&mut client, Duration::from_secs(2)).await;
    assert!(output.contains("PASTED_OK"), "bracketed paste content must execute: {output:?}");

    shutdown_server(&mut client, &mut server).await;
}

#[tokio::test]
async fn snapshot_includes_bracketed_paste_mode() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, mut server) = start_binary_server(&tmp).await;

    let mut client = TestClient::connect(&sock).await;
    let (sid, pid) = setup_attached_pane(&mut client).await;
    wait_for_prompt(&mut client).await;

    // Bash enables bracketed paste by default. Detach and reattach to get a snapshot.
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::DetachRuntime(v3::DetachRuntime {
                runtime_id: sid.clone(),
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::RuntimeDetached(_)) => break,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected RuntimeDetached, got {other:?}"),
        }
    }

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
                runtime_id: sid.clone(),
                attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    let snapshot = loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::RuntimeSnapshot(s)) => break s,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }
    };

    let pane =
        snapshot.panes.iter().find(|p| p.pane_id == pid).expect("pane missing from snapshot");
    assert!(
        pane.terminal_modes.as_ref().unwrap().bracketed_paste,
        "bash enables bracketed paste by default; snapshot must reflect this"
    );

    shutdown_server(&mut client, &mut server).await;
}

// ── Reconnect preserves terminal state ──────────────────────────

#[tokio::test]
async fn reconnect_preserves_command_output_in_scrollback() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, mut server) = start_binary_server(&tmp).await;

    let mut client = TestClient::connect(&sock).await;
    let (sid, pid) = setup_attached_pane(&mut client).await;
    wait_for_prompt(&mut client).await;

    send_input(&mut client, &sid, &pid, b"echo PERSIST_CHECK\r").await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let scrollback = reattach_snapshot_text(&mut client, &sid, &pid).await;
    assert!(
        scrollback.contains("PERSIST_CHECK"),
        "command output must survive detach/reattach: {scrollback:?}"
    );

    shutdown_server(&mut client, &mut server).await;
}

#[tokio::test]
async fn reconnect_allows_continued_input_after_reattach() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, mut server) = start_binary_server(&tmp).await;

    let mut client = TestClient::connect(&sock).await;
    let (sid, pid) = setup_attached_pane(&mut client).await;
    wait_for_prompt(&mut client).await;

    send_input(&mut client, &sid, &pid, b"echo BEFORE\r").await;
    wait_for_prompt(&mut client).await;

    // Detach and reattach.
    reattach_snapshot_text(&mut client, &sid, &pid).await;

    // Continue typing after reattach.
    send_input(&mut client, &sid, &pid, b"echo AFTER\r").await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let scrollback = reattach_snapshot_text(&mut client, &sid, &pid).await;
    assert!(
        scrollback.contains("BEFORE") && scrollback.contains("AFTER"),
        "input must work after reattach: {scrollback:?}"
    );

    shutdown_server(&mut client, &mut server).await;
}

// ── F-keys through daemon ───────────────────────────────────────

#[tokio::test]
async fn fkey_bytes_reach_pty_application() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, mut server) = start_binary_server(&tmp).await;

    let mut client = TestClient::connect(&sock).await;
    let (sid, pid) = setup_attached_pane(&mut client).await;
    wait_for_prompt(&mut client).await;

    // Pipe F1 escape sequence through cat -v to make it visible.
    send_input(&mut client, &sid, &pid, b"printf '\\033OP' | cat -v\r").await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let output = collect_output(&mut client, Duration::from_secs(2)).await;
    // cat -v renders ESC as ^[
    assert!(output.contains("^[OP"), "F1 escape sequence must reach PTY: {output:?}");

    shutdown_server(&mut client, &mut server).await;
}

// ── Mode restoration across reattach ────────────────────────────

async fn reattach_snapshot(client: &mut TestClient, runtime_id: &[u8]) -> v3::RuntimeSnapshot {
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::DetachRuntime(v3::DetachRuntime {
                runtime_id: runtime_id.to_vec(),
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::RuntimeDetached(_)) => break,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected RuntimeDetached, got {other:?}"),
        }
    }

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
                runtime_id: runtime_id.to_vec(),
                attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.payload {
            Some(v3::server_envelope::Payload::RuntimeSnapshot(s)) => return s,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn snapshot_includes_application_cursor_keys_mode() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, mut server) = start_binary_server(&tmp).await;

    let mut client = TestClient::connect(&sock).await;
    let (sid, pid) = setup_attached_pane(&mut client).await;
    wait_for_prompt(&mut client).await;

    // Enable application cursor keys (DECSET 1) via printf.
    send_input(&mut client, &sid, &pid, b"printf '\\033[?1h'\r").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    collect_output(&mut client, Duration::from_secs(1)).await;

    let snapshot = reattach_snapshot(&mut client, &sid).await;
    let pane = snapshot.panes.iter().find(|p| p.pane_id == pid).expect("pane missing");
    assert!(
        pane.terminal_modes.as_ref().unwrap().application_cursor_keys,
        "DECSET 1 must be reflected in snapshot"
    );

    shutdown_server(&mut client, &mut server).await;
}

#[tokio::test]
async fn snapshot_includes_application_keypad_mode() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, mut server) = start_binary_server(&tmp).await;

    let mut client = TestClient::connect(&sock).await;
    let (sid, pid) = setup_attached_pane(&mut client).await;
    wait_for_prompt(&mut client).await;

    // Enable application keypad (DECKPAM = ESC =) via printf.
    send_input(&mut client, &sid, &pid, b"printf '\\033='\r").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    collect_output(&mut client, Duration::from_secs(1)).await;

    let snapshot = reattach_snapshot(&mut client, &sid).await;
    let pane = snapshot.panes.iter().find(|p| p.pane_id == pid).expect("pane missing");
    assert!(
        pane.terminal_modes.as_ref().unwrap().application_keypad,
        "DECKPAM must be reflected in snapshot"
    );

    shutdown_server(&mut client, &mut server).await;
}

#[tokio::test]
async fn snapshot_includes_mouse_tracking_mode() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, mut server) = start_binary_server(&tmp).await;

    let mut client = TestClient::connect(&sock).await;
    let (sid, pid) = setup_attached_pane(&mut client).await;
    wait_for_prompt(&mut client).await;

    // Enable SGR mouse tracking (DECSET 1003 + 1006) via printf.
    send_input(&mut client, &sid, &pid, b"printf '\\033[?1003h\\033[?1006h'\r").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    collect_output(&mut client, Duration::from_secs(1)).await;

    let snapshot = reattach_snapshot(&mut client, &sid).await;
    let pane = snapshot.panes.iter().find(|p| p.pane_id == pid).expect("pane missing");
    assert_eq!(
        pane.terminal_modes.as_ref().unwrap().mouse_mode,
        v3::MouseMode::Any as i32,
        "DECSET 1003 must be reflected in snapshot"
    );
    assert!(
        pane.terminal_modes.as_ref().unwrap().sgr_mouse,
        "DECSET 1006 must be reflected in snapshot"
    );

    shutdown_server(&mut client, &mut server).await;
}

#[tokio::test]
async fn snapshot_modes_reset_when_disabled() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, mut server) = start_binary_server(&tmp).await;

    let mut client = TestClient::connect(&sock).await;
    let (sid, pid) = setup_attached_pane(&mut client).await;
    wait_for_prompt(&mut client).await;

    // Enable then disable application cursor keys.
    send_input(&mut client, &sid, &pid, b"printf '\\033[?1h'\r").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    send_input(&mut client, &sid, &pid, b"printf '\\033[?1l'\r").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    collect_output(&mut client, Duration::from_secs(1)).await;

    let snapshot = reattach_snapshot(&mut client, &sid).await;
    let pane = snapshot.panes.iter().find(|p| p.pane_id == pid).expect("pane missing");
    assert!(
        !pane.terminal_modes.as_ref().unwrap().application_cursor_keys,
        "DECRST 1 must clear the flag in snapshot"
    );

    shutdown_server(&mut client, &mut server).await;
}
