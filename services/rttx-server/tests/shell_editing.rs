//! End-to-end shell editing regression coverage.
//!
//! This exercises the exact daemon path used by managed workspaces:
//! a real `rttx-server` binary, a real interactive shell in a PTY,
//! protocol input bytes, and a rendered snapshot after the command runs.

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

    let mut command = Command::new(env!("CARGO_BIN_EXE_rttx-server"));
    command
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
        .kill_on_drop(true);
    let child = command.spawn().expect("failed to spawn rttx-server binary");

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
                name: "shell-editing".into(),
                policy: v3::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::RuntimeCreated(created)) => created.runtime_id,
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
        Some(v3::server_envelope::Payload::PaneCreated(created)) => created.pane_id,
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
        if let Some(message) = client.try_recv(Duration::from_millis(200)).await
            && let Some(v3::server_envelope::Payload::OutputDelta(delta)) = message.payload
        {
            output.extend(delta.data);
            if String::from_utf8_lossy(&output).contains(PROMPT) {
                return;
            }
        }
    }
    panic!("shell prompt did not arrive within 30 seconds");
}

async fn resize_pane(
    client: &mut TestClient,
    runtime_id: &[u8],
    pane_id: &[u8],
    cols: u32,
    rows: u32,
) {
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::ResizePane(v3::ResizePane {
                runtime_id: runtime_id.to_vec(),
                pane_id: pane_id.to_vec(),
                cols,
                rows,
            })),
        })
        .await;
    client.ping().await; // barrier: flush the fire-and-forget resize
}

fn pane_scrollback(snapshot: &v3::RuntimeSnapshot, pane_id: &[u8]) -> bytes::Bytes {
    snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == pane_id)
        .expect("pane missing from snapshot")
        .scrollback_tail
        .clone()
}

async fn reattach_snapshot_bytes(
    client: &mut TestClient,
    runtime_id: &[u8],
    pane_id: &[u8],
) -> bytes::Bytes {
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
            Some(v3::server_envelope::Payload::RuntimeSnapshot(snapshot)) => break snapshot,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }
    };
    pane_scrollback(&snapshot, pane_id)
}

async fn snapshot_scrollback(client: &mut TestClient, runtime_id: &[u8], pane_id: &[u8]) -> String {
    normalize_scrollback(&reattach_snapshot_bytes(client, runtime_id, pane_id).await)
}

async fn attach_snapshot_bytes(
    client: &mut TestClient,
    runtime_id: &[u8],
    pane_id: &[u8],
) -> bytes::Bytes {
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
            Some(v3::server_envelope::Payload::RuntimeSnapshot(snapshot)) => break snapshot,
            Some(v3::server_envelope::Payload::OutputDelta(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }
    };
    pane_scrollback(&snapshot, pane_id)
}

async fn attach_and_collect_prompt(
    client: &mut TestClient,
    runtime_id: &[u8],
    pane_id: &[u8],
) -> String {
    let mut output = attach_snapshot_bytes(client, runtime_id, pane_id).await.to_vec();
    if normalize_scrollback(&output).ends_with(PROMPT) {
        return normalize_scrollback(&output);
    }

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        if let Some(message) = client.try_recv(Duration::from_millis(200)).await
            && let Some(v3::server_envelope::Payload::OutputDelta(delta)) = message.payload
        {
            output.extend(&delta.data);
            if normalize_scrollback(&output).ends_with(PROMPT) {
                return normalize_scrollback(&output);
            }
        }
    }

    panic!("shell prompt did not arrive after reattach");
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

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

async fn shutdown_server(client: &mut TestClient, server_child: &mut Child) {
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::Shutdown(v3::Shutdown {})),
        })
        .await;
    let status = tokio::time::timeout(Duration::from_secs(5), server_child.wait())
        .await
        .expect("timed out waiting for rttx-server to stop")
        .expect("failed to wait for rttx-server child");
    assert!(status.success(), "rttx-server exited unsuccessfully: {status}");
}

fn normalize_scrollback(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .replace("\x1b[?2004h", "")
        .replace("\x1b[?2004l", "")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

#[tokio::test]
async fn shell_line_editing_bytes_render_expected_output_after_snapshot_restore() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, mut server_child) = start_binary_server(&tmp).await;

    let mut client = TestClient::connect(&socket_path).await;
    let (runtime_id, pane_id) = setup_attached_pane(&mut client).await;
    wait_for_prompt(&mut client).await;

    // Type `echo abxd`, move left once, backspace the `x`, insert `c`,
    // then press Return. This should execute `echo abcd`.
    send_input(&mut client, &runtime_id, &pane_id, b"echo abxd\x1b[D\x7fc\r").await;

    tokio::time::sleep(Duration::from_millis(800)).await;
    let scrollback = snapshot_scrollback(&mut client, &runtime_id, &pane_id).await;

    assert!(
        scrollback.contains("\nabcd\nPROMPT> "),
        "expected rendered output line and prompt after edited command.\nscrollback:\n{scrollback:?}"
    );

    shutdown_server(&mut client, &mut server_child).await;
}

#[tokio::test]
async fn shell_line_editing_survives_detach_mid_command_and_executes_after_reattach() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, mut server_child) = start_binary_server(&tmp).await;

    let mut client = TestClient::connect(&socket_path).await;
    let (runtime_id, pane_id) = setup_attached_pane(&mut client).await;
    wait_for_prompt(&mut client).await;

    send_input(&mut client, &runtime_id, &pane_id, b"echo abxd\x1b[D\x7f").await;
    tokio::time::sleep(Duration::from_millis(250)).await;

    let partial_snapshot = reattach_snapshot_bytes(&mut client, &runtime_id, &pane_id).await;
    assert!(
        contains_bytes(&partial_snapshot, b"echo abxd"),
        "expected the in-progress command to survive reattach.\nsnapshot bytes: {:?}",
        String::from_utf8_lossy(&partial_snapshot)
    );

    send_input(&mut client, &runtime_id, &pane_id, b"c\r").await;
    tokio::time::sleep(Duration::from_millis(800)).await;

    let scrollback = snapshot_scrollback(&mut client, &runtime_id, &pane_id).await;
    assert!(
        scrollback.contains("\nabcd\nPROMPT> "),
        "expected reattached line editing to continue at the restored cursor.\nscrollback:\n{scrollback:?}"
    );

    shutdown_server(&mut client, &mut server_child).await;
}

#[tokio::test]
async fn wrapped_shell_line_editing_survives_detach_and_reattach() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, mut server_child) = start_binary_server(&tmp).await;

    let mut client = TestClient::connect(&socket_path).await;
    let (runtime_id, pane_id) = setup_attached_pane(&mut client).await;
    wait_for_prompt(&mut client).await;
    resize_pane(&mut client, &runtime_id, &pane_id, 12, 24).await;

    send_input(&mut client, &runtime_id, &pane_id, b"echo 0123456789abxd\x1b[D\x7f").await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    reattach_snapshot_bytes(&mut client, &runtime_id, &pane_id).await;

    send_input(&mut client, &runtime_id, &pane_id, b"c\r").await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let scrollback = snapshot_scrollback(&mut client, &runtime_id, &pane_id).await;
    assert!(
        scrollback.contains("0123456789abcd\nPROMPT> "),
        "expected wrapped line editing to survive reattach with the cursor in the right column.\nscrollback:\n{scrollback:?}"
    );

    shutdown_server(&mut client, &mut server_child).await;
}

#[tokio::test]
async fn formatted_output_survives_reattach_and_allows_follow_up_input() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, mut server_child) = start_binary_server(&tmp).await;

    let mut client = TestClient::connect(&socket_path).await;
    let (runtime_id, pane_id) = setup_attached_pane(&mut client).await;
    wait_for_prompt(&mut client).await;

    send_input(&mut client, &runtime_id, &pane_id, b"printf $'\\033[31mRED\\033[0m\\n'\r").await;
    tokio::time::sleep(Duration::from_millis(600)).await;

    let snapshot_bytes = reattach_snapshot_bytes(&mut client, &runtime_id, &pane_id).await;
    assert!(
        contains_bytes(&snapshot_bytes, b"\x1b[31mRED\x1b[0m"),
        "expected ANSI formatting bytes to survive snapshot replay.\nsnapshot bytes: {:?}",
        String::from_utf8_lossy(&snapshot_bytes)
    );

    send_input(&mut client, &runtime_id, &pane_id, b"echo AFTER\r").await;
    tokio::time::sleep(Duration::from_millis(600)).await;

    let scrollback = snapshot_scrollback(&mut client, &runtime_id, &pane_id).await;
    assert!(
        scrollback.contains("RED") && scrollback.contains("AFTER\nPROMPT> "),
        "expected formatted output and follow-up typing to survive reattach.\nscrollback:\n{scrollback:?}"
    );

    shutdown_server(&mut client, &mut server_child).await;
}

#[test]
fn graceful_restart_does_not_fossilize_stale_prompt_lines() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let tmp = tempfile::TempDir::new().unwrap();
        let (socket_path, mut server_child) = start_binary_server(&tmp).await;

        let mut client = TestClient::connect(&socket_path).await;
        let (runtime_id, pane_id) = setup_attached_pane(&mut client).await;
        wait_for_prompt(&mut client).await;

        send_input(&mut client, &runtime_id, &pane_id, b"echo cycle-0\r").await;
        wait_for_prompt(&mut client).await;
        shutdown_server(&mut client, &mut server_child).await;

        let (socket_path, mut server_child) = start_binary_server(&tmp).await;
        let mut client = TestClient::connect(&socket_path).await;
        client.handshake().await;

        let first_restart = attach_and_collect_prompt(&mut client, &runtime_id, &pane_id).await;
        assert!(
            !first_restart.contains("PROMPT> PROMPT> "),
            "first restart should not replay stacked prompts.\nscrollback:\n{first_restart:?}"
        );

        send_input(&mut client, &runtime_id, &pane_id, b"echo cycle-1\r").await;
        wait_for_prompt(&mut client).await;
        shutdown_server(&mut client, &mut server_child).await;

        let (socket_path, mut server_child) = start_binary_server(&tmp).await;
        let mut client = TestClient::connect(&socket_path).await;
        client.handshake().await;

        let second_restart = attach_and_collect_prompt(&mut client, &runtime_id, &pane_id).await;
        assert!(
            !second_restart.contains("PROMPT> PROMPT> "),
            "repeated graceful restarts must not fossilize stale prompt tails.\nscrollback:\n{second_restart:?}"
        );
        assert!(
            second_restart.contains("cycle-0\n") && second_restart.contains("cycle-1\n"),
            "restored scrollback should retain completed command output across restarts.\nscrollback:\n{second_restart:?}"
        );
        assert!(
            second_restart.ends_with(PROMPT),
            "restored shell should end on a single live prompt after restart.\nscrollback:\n{second_restart:?}"
        );

        shutdown_server(&mut client, &mut server_child).await;
    });
}

#[tokio::test]
async fn reattach_without_resize_does_not_duplicate_prompt() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, mut server_child) = start_binary_server(&tmp).await;

    let mut client = TestClient::connect(&socket_path).await;
    let (runtime_id, pane_id) = setup_attached_pane(&mut client).await;
    wait_for_prompt(&mut client).await;

    // Detach and reattach without sending a resize.
    let scrollback = reattach_snapshot_bytes(&mut client, &runtime_id, &pane_id).await;
    let text = normalize_scrollback(&scrollback);

    // The snapshot should end with exactly one prompt, not a duplicate.
    assert!(
        !text.contains("PROMPT> PROMPT> "),
        "reattach snapshot should not contain duplicated prompts.\nscrollback:\n{text:?}"
    );
    assert!(
        text.ends_with(PROMPT),
        "reattach snapshot should end with a single prompt.\nscrollback:\n{text:?}"
    );

    shutdown_server(&mut client, &mut server_child).await;
}
