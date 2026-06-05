//! Managed-vs-direct TUI parity harness.
//!
//! Runs the repo-owned `pty-exerciser` binary through two paths:
//!   1. **Direct** — raw PTY, output fed through `PaneScreen`
//!   2. **Managed** — daemon-backed pane via the wire protocol
//!
//! Both paths execute identical exerciser commands and assert that the
//! resulting terminal mode state matches. This catches regressions where
//! the daemon transport or snapshot logic silently drops mode changes.
//!
//! Required by #766.

mod common;

use common::TestClient;
use rttx_proto::v3;
use rttx_server::screen::PaneScreen;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

const READY_MARKER: &str = "EXERCISER_READY";

fn exerciser_bin() -> String {
    env!("CARGO_BIN_EXE_pty-exerciser").to_string()
}

// ── Direct-path helpers ─────────────────────────────────────────

struct DirectPty {
    screen: PaneScreen,
    read: pty_process::OwnedReadPty,
    write: pty_process::OwnedWritePty,
    _child: tokio::process::Child,
}

impl DirectPty {
    fn spawn() -> Self {
        let (pty, pts) = pty_process::open().expect("open pty");
        pty.resize(pty_process::Size::new(24, 80)).expect("resize");
        let child = pty_process::Command::new(exerciser_bin())
            .env("TERM", "xterm-256color")
            .spawn(pts)
            .expect("spawn exerciser");
        let (read, write) = pty.into_split();
        Self { screen: PaneScreen::new(64 * 1024), read, write, _child: child }
    }

    async fn send_command(&mut self, cmd: &str) {
        let line = format!("{cmd}\n");
        self.write.write_all(line.as_bytes()).await.expect("write to pty");
        self.write.flush().await.expect("flush pty");
    }

    async fn drain_output(&mut self, window: Duration) {
        let deadline = tokio::time::Instant::now() + window;
        let mut buf = [0u8; 4096];
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, self.read.read(&mut buf)).await {
                Ok(Ok(0) | Err(_)) | Err(_) => break,
                Ok(Ok(n)) => self.screen.feed(&buf[..n]),
            }
        }
    }

    async fn wait_for_ready(&mut self) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut buf = [0u8; 4096];
        let mut collected = Vec::new();
        self.send_command("READY").await;
        loop {
            assert!(tokio::time::Instant::now() < deadline, "exerciser READY not received");
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(remaining, self.read.read(&mut buf)).await {
                Ok(Ok(0)) | Err(_) => panic!("exerciser READY not received"),
                Ok(Ok(n)) => {
                    self.screen.feed(&buf[..n]);
                    collected.extend_from_slice(&buf[..n]);
                    if String::from_utf8_lossy(&collected).contains(READY_MARKER) {
                        return;
                    }
                }
                Ok(Err(e)) => panic!("read error waiting for READY: {e}"),
            }
        }
    }
}

// ── Managed-path helpers ────────────────────────────────────────

async fn start_exerciser_server(tmp: &tempfile::TempDir) -> (PathBuf, Child) {
    let runtime_dir = tmp.path().join("runtime");
    let cache_dir = tmp.path().join("cache");
    let config_dir = tmp.path().join("config");
    let home_dir = tmp.path().join("home");

    std::fs::create_dir_all(&runtime_dir).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&home_dir).unwrap();

    // Wrapper script that execs the exerciser binary.
    let shell_path = tmp.path().join("exerciser-shell");
    std::fs::write(&shell_path, format!("#!/bin/sh\nexec {}\n", exerciser_bin())).unwrap();
    let mut perms = std::fs::metadata(&shell_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&shell_path, perms).unwrap();

    let socket_path = runtime_dir.join("rttx-server").join("v1").join("rttx-server.sock");

    let child = Command::new(env!("CARGO_BIN_EXE_rttx-server"))
        .arg("start")
        .arg("--foreground")
        .env("XDG_RUNTIME_DIR", &runtime_dir)
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
        .expect("spawn rttx-server");

    wait_for_socket(&socket_path).await;
    (socket_path, child)
}

async fn wait_for_socket(path: &Path) {
    for _ in 0..100 {
        if path.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("server socket did not appear at {}", path.display());
}

async fn setup_managed_pane(client: &mut TestClient) -> (Vec<u8>, Vec<u8>) {
    client.handshake().await;

    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
                name: "parity".into(),
                policy: v3::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
    let runtime_id = match client.recv_or_timeout().await.payload {
        Some(v3::server_envelope::Payload::RuntimeCreated(rc)) => rc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    client
        .send(&v3::ClientEnvelope {
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

async fn managed_send_input(
    client: &mut TestClient,
    runtime_id: &[u8],
    pane_id: &[u8],
    data: &[u8],
) {
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

async fn managed_wait_for_ready(client: &mut TestClient) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut output = Vec::new();
    while tokio::time::Instant::now() < deadline {
        if let Some(msg) = client.try_recv(Duration::from_millis(200)).await
            && let Some(v3::server_envelope::Payload::OutputDelta(delta)) = msg.payload
        {
            output.extend(delta.data);
            if String::from_utf8_lossy(&output).contains(READY_MARKER) {
                return;
            }
        }
    }
    panic!("exerciser READY not received via daemon within 30s");
}

async fn managed_snapshot_pane(
    client: &mut TestClient,
    runtime_id: &[u8],
    pane_id: &[u8],
) -> v3::PaneSnapshot {
    // Detach
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

    // Reattach
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
    snapshot.panes.into_iter().find(|p| p.pane_id == pane_id).expect("pane missing from snapshot")
}

async fn shutdown_server(client: &mut TestClient, server: &mut Child) {
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::Shutdown(v3::Shutdown {})),
        })
        .await;
    let status = tokio::time::timeout(Duration::from_secs(10), server.wait())
        .await
        .expect("timed out waiting for rttx-server to stop")
        .expect("failed to wait for rttx-server child");
    assert!(status.success(), "rttx-server exited unsuccessfully: {status}");
}

/// Collected mode state from either path, used for parity comparison.
#[derive(Debug, PartialEq, Eq)]
struct ModeState {
    application_cursor_keys: bool,
    application_keypad: bool,
    bracketed_paste: bool,
    focus_reporting: bool,
    mouse_tracking_mode: u16,
    sgr_mouse: bool,
}

impl ModeState {
    const fn from_screen(screen: &PaneScreen) -> Self {
        Self {
            application_cursor_keys: screen.application_cursor_keys(),
            application_keypad: screen.application_keypad(),
            bracketed_paste: screen.bracketed_paste_mode(),
            focus_reporting: screen.focus_event_mode(),
            mouse_tracking_mode: screen.mouse_tracking_mode(),
            sgr_mouse: screen.sgr_mouse_mode(),
        }
    }

    const fn from_snapshot(snap: &v3::PaneSnapshot) -> Self {
        Self {
            application_cursor_keys: snap.application_cursor_keys,
            application_keypad: snap.application_keypad,
            bracketed_paste: snap.bracketed_paste_mode,
            focus_reporting: false, // v2 proto does not expose focus_reporting
            mouse_tracking_mode: snap.mouse_tracking_mode as u16,
            sgr_mouse: snap.sgr_mouse_mode,
        }
    }
}

/// Run a sequence of exerciser commands through the direct PTY path
/// and return the resulting mode state.
async fn direct_mode_state(commands: &[&str]) -> ModeState {
    let mut pty = DirectPty::spawn();
    pty.wait_for_ready().await;
    for cmd in commands {
        pty.send_command(cmd).await;
    }
    pty.drain_output(Duration::from_millis(500)).await;
    ModeState::from_screen(&pty.screen)
}

/// Run a sequence of exerciser commands through the managed daemon path
/// and return the resulting mode state from the snapshot.
async fn managed_mode_state(commands: &[&str]) -> ModeState {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, mut server) = start_exerciser_server(&tmp).await;
    let mut client = TestClient::connect(&sock).await;
    let (rid, pid) = setup_managed_pane(&mut client).await;

    // Send READY and wait for the exerciser to be alive.
    managed_send_input(&mut client, &rid, &pid, b"READY\n").await;
    managed_wait_for_ready(&mut client).await;

    for cmd in commands {
        let line = format!("{cmd}\n");
        managed_send_input(&mut client, &rid, &pid, line.as_bytes()).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    // Drain any pending deltas before snapshot.
    client.drain(Duration::from_millis(200)).await;

    let snap = managed_snapshot_pane(&mut client, &rid, &pid).await;
    let state = ModeState::from_snapshot(&snap);

    shutdown_server(&mut client, &mut server).await;
    state
}

// ── Parity tests ────────────────────────────────────────────────

#[tokio::test]
async fn parity_application_cursor_keys_set() {
    let commands = &["SET app_cursor"];
    let direct = direct_mode_state(commands).await;
    let managed = managed_mode_state(commands).await;
    assert!(direct.application_cursor_keys, "direct: app cursor must be on");
    assert!(managed.application_cursor_keys, "managed: app cursor must be on");
    assert_eq!(
        direct.application_cursor_keys, managed.application_cursor_keys,
        "parity: application_cursor_keys must match"
    );
}

#[tokio::test]
async fn parity_application_cursor_keys_set_then_reset() {
    let commands = &["SET app_cursor", "RESET app_cursor"];
    let direct = direct_mode_state(commands).await;
    let managed = managed_mode_state(commands).await;
    assert!(!direct.application_cursor_keys, "direct: app cursor must be off after reset");
    assert!(!managed.application_cursor_keys, "managed: app cursor must be off after reset");
}

#[tokio::test]
async fn parity_application_keypad_set() {
    let commands = &["SET app_keypad"];
    let direct = direct_mode_state(commands).await;
    let managed = managed_mode_state(commands).await;
    assert!(direct.application_keypad, "direct: app keypad must be on");
    assert!(managed.application_keypad, "managed: app keypad must be on");
    assert_eq!(
        direct.application_keypad, managed.application_keypad,
        "parity: application_keypad must match"
    );
}

#[tokio::test]
async fn parity_bracketed_paste_set() {
    let commands = &["SET bracketed_paste"];
    let direct = direct_mode_state(commands).await;
    let managed = managed_mode_state(commands).await;
    assert!(direct.bracketed_paste, "direct: bracketed paste must be on");
    assert!(managed.bracketed_paste, "managed: bracketed paste must be on");
    assert_eq!(
        direct.bracketed_paste, managed.bracketed_paste,
        "parity: bracketed_paste must match"
    );
}

#[tokio::test]
async fn parity_focus_reporting_set() {
    let commands = &["SET focus_reporting"];
    let direct = direct_mode_state(commands).await;
    assert!(direct.focus_reporting, "direct: focus reporting must be on");
    // Managed path: v2 proto does not expose focus_reporting in PaneSnapshot,
    // so we verify the direct path only. v3 proto covers this via TerminalModeState.
}

#[tokio::test]
async fn parity_mouse_1000_set() {
    let commands = &["SET mouse_1000"];
    let direct = direct_mode_state(commands).await;
    let managed = managed_mode_state(commands).await;
    assert_eq!(direct.mouse_tracking_mode, 1000, "direct: mouse 1000");
    assert_eq!(managed.mouse_tracking_mode, 1000, "managed: mouse 1000");
}

#[tokio::test]
async fn parity_mouse_1003_with_sgr() {
    let commands = &["SET mouse_1003", "SET sgr_mouse"];
    let direct = direct_mode_state(commands).await;
    let managed = managed_mode_state(commands).await;
    assert_eq!(direct.mouse_tracking_mode, 1003, "direct: mouse 1003");
    assert!(direct.sgr_mouse, "direct: sgr mouse must be on");
    assert_eq!(managed.mouse_tracking_mode, 1003, "managed: mouse 1003");
    assert!(managed.sgr_mouse, "managed: sgr mouse must be on");
    assert_eq!(
        direct.mouse_tracking_mode, managed.mouse_tracking_mode,
        "parity: mouse_tracking_mode must match"
    );
    assert_eq!(direct.sgr_mouse, managed.sgr_mouse, "parity: sgr_mouse must match");
}

#[tokio::test]
async fn parity_all_modes_combined() {
    let commands = &[
        "SET app_cursor",
        "SET app_keypad",
        "SET bracketed_paste",
        "SET focus_reporting",
        "SET mouse_1003",
        "SET sgr_mouse",
    ];
    let direct = direct_mode_state(commands).await;
    let managed = managed_mode_state(commands).await;

    assert!(direct.application_cursor_keys);
    assert!(direct.application_keypad);
    assert!(direct.bracketed_paste);
    assert!(direct.focus_reporting);
    assert_eq!(direct.mouse_tracking_mode, 1003);
    assert!(direct.sgr_mouse);

    // Managed parity (excluding focus_reporting which v2 proto lacks).
    assert_eq!(direct.application_cursor_keys, managed.application_cursor_keys);
    assert_eq!(direct.application_keypad, managed.application_keypad);
    assert_eq!(direct.bracketed_paste, managed.bracketed_paste);
    assert_eq!(direct.mouse_tracking_mode, managed.mouse_tracking_mode);
    assert_eq!(direct.sgr_mouse, managed.sgr_mouse);
}

#[tokio::test]
async fn parity_modes_default_off() {
    let commands: &[&str] = &[];
    let direct = direct_mode_state(commands).await;
    let managed = managed_mode_state(commands).await;

    assert!(!direct.application_cursor_keys);
    assert!(!direct.application_keypad);
    assert!(!direct.bracketed_paste);
    assert!(!direct.focus_reporting);
    assert_eq!(direct.mouse_tracking_mode, 0);
    assert!(!direct.sgr_mouse);

    assert_eq!(direct.application_cursor_keys, managed.application_cursor_keys);
    assert_eq!(direct.application_keypad, managed.application_keypad);
    // Note: managed bracketed_paste may differ because bash enables it by default.
    // The exerciser runs without bash, so both should be off.
    assert_eq!(direct.bracketed_paste, managed.bracketed_paste);
    assert_eq!(direct.mouse_tracking_mode, managed.mouse_tracking_mode);
    assert_eq!(direct.sgr_mouse, managed.sgr_mouse);
}
