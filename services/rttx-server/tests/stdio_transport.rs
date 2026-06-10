//! Integration test for the attach-stdio transport.
//!
//! Verifies that the protocol works over a pipe (simulating SSH stdio)
//! by starting a daemon, then spawning `attach-stdio` as a proxy and
//! communicating over its stdin/stdout.

use bytes::BytesMut;
use rttx_proto::{bytes_to_uuid, decode_frame, encode_frame, v3};
use std::process::Stdio;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

async fn start_daemon(
    bin: &str,
    runtime_dir: &std::path::Path,
    cache_dir: &std::path::Path,
    state_dir: &std::path::Path,
) -> tokio::process::Child {
    let child = Command::new(bin)
        .arg("start")
        .arg("--foreground")
        .env("XDG_RUNTIME_DIR", runtime_dir)
        .env("RTTX_DEV_MODE", "")
        .env("XDG_CACHE_HOME", cache_dir)
        .env("XDG_STATE_HOME", state_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to spawn daemon");

    // Wait for socket to appear.
    let socket = runtime_dir.join("rttx-server").join("v1").join("rttx-server.sock");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        if socket.exists() {
            return child;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("daemon socket did not appear at {}", socket.display());
}

async fn read_response(
    stdout: &mut tokio::process::ChildStdout,
    read_buf: &mut BytesMut,
) -> v3::ServerEnvelope {
    loop {
        let n = stdout.read_buf(read_buf).await.unwrap();
        assert!(n > 0, "unexpected EOF");
        match decode_frame::<v3::ServerEnvelope>(read_buf) {
            Ok(msg) => return msg,
            Err(rttx_proto::FrameError::Incomplete) => {}
            Err(e) => panic!("decode error: {e}"),
        }
    }
}

/// Spawn `rttx-server attach-stdio` against a running daemon and speak the protocol.
#[tokio::test]
async fn attach_stdio_hello_and_create_runtime() {
    let bin = env!("CARGO_BIN_EXE_rttx-server");
    let tmp = TempDir::new().unwrap();
    let runtime_dir = tmp.path().join("runtime");
    let cache_dir = tmp.path().join("cache");
    let state_dir = tmp.path().join("state");
    tokio::fs::create_dir_all(&runtime_dir).await.unwrap();
    tokio::fs::create_dir_all(&cache_dir).await.unwrap();
    tokio::fs::create_dir_all(&state_dir).await.unwrap();

    let mut daemon = start_daemon(bin, &runtime_dir, &cache_dir, &state_dir).await;

    let mut child = Command::new(bin)
        .arg("attach-stdio")
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("RTTX_DEV_MODE", "")
        .env("XDG_CACHE_HOME", &cache_dir)
        .env("XDG_STATE_HOME", &state_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to spawn attach-stdio");

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut read_buf = BytesMut::with_capacity(4096);

    // v3 handshake.
    let hello = rttx_proto::v3_handshake::build_client_hello(
        uuid::Uuid::new_v4(),
        "test-stdio",
        "0.0.0",
        rttx_proto::v3_handshake::CORE_CAPABILITIES,
    );
    let mut buf = BytesMut::new();
    encode_frame(&hello, &mut buf).unwrap();
    stdin.write_all(&buf).await.unwrap();
    stdin.flush().await.unwrap();

    // Read ServerHello (bare frame).
    loop {
        let n = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            stdout.read_buf(&mut read_buf),
        )
        .await
        .expect("timed out")
        .expect("read failed");
        assert!(n > 0, "unexpected EOF");
        if decode_frame::<v3::ServerHello>(&mut read_buf).is_ok() {
            break;
        }
    }

    // Create session.
    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
            name: "stdio-test".into(),
            policy: v3::RuntimePolicy::Persistent as i32,
        })),
    };
    buf.clear();
    encode_frame(&create, &mut buf).unwrap();
    stdin.write_all(&buf).await.unwrap();
    stdin.flush().await.unwrap();

    let resp = read_response(&mut stdout, &mut read_buf).await;
    let runtime_id = match resp.payload {
        Some(v3::server_envelope::Payload::RuntimeCreated(sc)) => {
            bytes_to_uuid(&sc.runtime_id).unwrap()
        }
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };
    assert!(!runtime_id.is_nil());

    // List runtimes.
    let list = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::ListRuntimes(v3::ListRuntimes {})),
    };
    buf.clear();
    encode_frame(&list, &mut buf).unwrap();
    stdin.write_all(&buf).await.unwrap();
    stdin.flush().await.unwrap();

    let resp = read_response(&mut stdout, &mut read_buf).await;
    match resp.payload {
        Some(v3::server_envelope::Payload::RuntimeList(sl)) => {
            assert_eq!(sl.runtimes.len(), 1);
            assert_eq!(sl.runtimes[0].name, "stdio-test");
        }
        other => panic!("expected RuntimeList, got {other:?}"),
    }

    // Disconnect — process should exit.
    drop(stdin);
    let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
        .await
        .expect("timed out")
        .expect("wait failed");
    assert!(status.success() || status.code() == Some(0));

    daemon.kill().await.ok();
}

/// Gate evidence: attach-stdio is a proxy, not a standalone server.
#[test]
fn attach_stdio_requires_running_daemon() {
    let bin = env!("CARGO_BIN_EXE_rttx-server");
    let tmp = tempfile::TempDir::new().unwrap();
    let runtime_dir = tmp.path().join("runtime");
    let cache_dir = tmp.path().join("cache");
    std::fs::create_dir_all(&runtime_dir).unwrap();
    std::fs::create_dir_all(&cache_dir).unwrap();

    // No daemon running — attach-stdio should fail.
    let output = std::process::Command::new(bin)
        .arg("attach-stdio")
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("RTTX_DEV_MODE", "")
        .env("XDG_CACHE_HOME", &cache_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("failed to run attach-stdio");

    assert!(!output.status.success(), "attach-stdio must fail without a running daemon");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("daemon socket not found") || stderr.contains("socket"),
        "error should mention missing socket, got: {stderr}"
    );
}

/// The status command must show version and socket path even when daemon is not running.
#[test]
fn status_command_shows_not_running_when_no_daemon() {
    let bin = env!("CARGO_BIN_EXE_rttx-server");
    let tmp = tempfile::TempDir::new().unwrap();
    let runtime_dir = tmp.path().join("runtime");
    std::fs::create_dir_all(&runtime_dir).unwrap();

    let output = std::process::Command::new(bin)
        .arg("status")
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("RTTX_DEV_MODE", "")
        .output()
        .expect("failed to run status");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("rttx-server"), "must show version line");
    assert!(stdout.contains("not running"), "must report not running");
}

/// Version string must include git hash. Regression for version tracking.
#[test]
fn version_includes_git_hash() {
    let bin = env!("CARGO_BIN_EXE_rttx-server");
    let output =
        std::process::Command::new(bin).arg("--version").output().expect("failed to run --version");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("rttx-server"), "must show binary name");
    assert!(stdout.contains('('), "must include git hash in parens");
}
