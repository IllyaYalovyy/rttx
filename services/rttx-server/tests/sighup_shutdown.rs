//! Test that the daemon handles SIGHUP gracefully (shuts down instead of dying silently).

use std::process::Stdio;
use std::time::Duration;
use tempfile::TempDir;
use tokio::process::Command;

#[tokio::test]
async fn sighup_triggers_graceful_shutdown() {
    let tmp = TempDir::new().unwrap();
    let runtime_dir = tmp.path().join("workspace");
    let cache_dir = tmp.path().join("cache");
    let state_dir = tmp.path().join("state");
    tokio::fs::create_dir_all(&runtime_dir).await.unwrap();
    tokio::fs::create_dir_all(&cache_dir).await.unwrap();
    tokio::fs::create_dir_all(&state_dir).await.unwrap();

    let bin = env!("CARGO_BIN_EXE_rttx-server");
    let mut child = Command::new(bin)
        .arg("start")
        .arg("--foreground")
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("RTTX_DEV_MODE", "")
        .env("XDG_CACHE_HOME", &cache_dir)
        .env("XDG_STATE_HOME", &state_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn daemon");

    let socket = runtime_dir.join("rttx-server").join("v1").join("rttx-server.sock");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(socket.exists(), "daemon socket must appear");

    // Send SIGHUP.
    let pid = child.id().expect("child pid");
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid as i32),
        nix::sys::signal::Signal::SIGHUP,
    )
    .expect("kill SIGHUP");

    // Daemon should exit gracefully within a few seconds.
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("daemon must exit within 10s after SIGHUP")
        .expect("wait failed");

    assert!(status.success(), "daemon must exit cleanly on SIGHUP, got: {status}");
}
