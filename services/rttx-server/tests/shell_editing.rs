//! End-to-end shell editing regression coverage.
//!
//! This exercises the exact daemon path used by managed workspaces:
//! a real `rttx-server` binary, a real interactive shell in a PTY,
//! protocol input bytes, and a rendered snapshot after the command runs.

mod common;

use common::TestClient;
use rttx_proto::proto;
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
        .env("XDG_CACHE_HOME", &cache_dir)
        .env("XDG_CONFIG_HOME", &config_dir)
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
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
                name: "shell-editing".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let session_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::SessionCreated(created)) => created.session_id,
        other => panic!("expected SessionCreated, got {other:?}"),
    };

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
                session_id: session_id.clone(),
            })),
        })
        .await;
    let pane_id = match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::PaneCreated(created)) => created.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.clone(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    match client.recv_or_timeout().await.msg {
        Some(proto::server_message::Msg::Snapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }

    (session_id, pane_id)
}

async fn wait_for_prompt(client: &mut TestClient) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    while tokio::time::Instant::now() < deadline {
        if let Some(message) = client.try_recv(Duration::from_millis(200)).await
            && let Some(proto::server_message::Msg::Delta(delta)) = message.msg
        {
            output.extend(delta.data);
            if String::from_utf8_lossy(&output).contains(PROMPT) {
                return;
            }
        }
    }
    panic!("shell prompt did not arrive within 5 seconds");
}

async fn snapshot_scrollback(client: &mut TestClient, session_id: &[u8], pane_id: &[u8]) -> String {
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::DetachSession(proto::DetachSession {
                session_id: session_id.to_vec(),
            })),
        })
        .await;
    loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::SessionDetached(_)) => break,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected SessionDetached, got {other:?}"),
        }
    }

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
                session_id: session_id.to_vec(),
                attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
            })),
        })
        .await;
    let snapshot = loop {
        match client.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::Snapshot(snapshot)) => break snapshot,
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected Snapshot, got {other:?}"),
        }
    };
    let pane = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == pane_id)
        .expect("pane missing from snapshot");
    normalize_scrollback(&pane.scrollback)
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
    let (session_id, pane_id) = setup_attached_pane(&mut client).await;
    wait_for_prompt(&mut client).await;

    // Type `echo abxd`, move left once, backspace the `x`, insert `c`,
    // then press Return. This should execute `echo abcd`.
    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Input(proto::Input {
                session_id: session_id.clone(),
                pane_id: pane_id.clone(),
                data: b"echo abxd\x1b[D\x7fc\r".to_vec(),
            })),
        })
        .await;

    tokio::time::sleep(Duration::from_millis(800)).await;
    let scrollback = snapshot_scrollback(&mut client, &session_id, &pane_id).await;

    assert!(
        scrollback.contains("\nabcd\nPROMPT> "),
        "expected rendered output line and prompt after edited command.\nscrollback:\n{scrollback:?}"
    );

    client
        .send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::Shutdown(proto::Shutdown {})),
        })
        .await;
    let status = tokio::time::timeout(Duration::from_secs(5), server_child.wait())
        .await
        .expect("timed out waiting for rttx-server to stop")
        .expect("failed to wait for rttx-server child");
    assert!(status.success(), "rttx-server exited unsuccessfully: {status}");
}
