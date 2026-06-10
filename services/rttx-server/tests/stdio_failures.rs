//! Attach-stdio failure-path tests.
//!
//! Exercises failure modes of the stdio transport: peer disconnect,
//! broken pipe, handshake mismatch, and garbage input.

use bytes::BytesMut;
use rttx_proto::{decode_frame, encode_frame, uuid_to_bytes, v3};
use std::process::Stdio;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

async fn spawn_daemon(tmp: &TempDir) -> Child {
    let bin = env!("CARGO_BIN_EXE_rttx-server");
    let runtime_dir = tmp.path().join("runtime");
    let cache_dir = tmp.path().join("cache");
    tokio::fs::create_dir_all(&runtime_dir).await.unwrap();
    tokio::fs::create_dir_all(&cache_dir).await.unwrap();

    let child = Command::new(bin)
        .arg("start")
        .arg("--foreground")
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("RTTX_DEV_MODE", "")
        .env("XDG_CACHE_HOME", &cache_dir)
        .env("XDG_STATE_HOME", tmp.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to spawn daemon");

    let socket = runtime_dir.join("rttx-server").join("v1").join("rttx-server.sock");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        if socket.exists() {
            return child;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("daemon socket did not appear");
}

fn spawn_stdio(tmp: &TempDir) -> Child {
    let bin = env!("CARGO_BIN_EXE_rttx-server");
    let runtime_dir = tmp.path().join("runtime");
    let cache_dir = tmp.path().join("cache");

    Command::new(bin)
        .arg("attach-stdio")
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("RTTX_DEV_MODE", "")
        .env("XDG_CACHE_HOME", &cache_dir)
        .env("XDG_STATE_HOME", tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to spawn rttx-server attach-stdio")
}

async fn send_frame<M: prost::Message>(stdin: &mut tokio::process::ChildStdin, msg: &M) {
    let mut buf = BytesMut::new();
    encode_frame(msg, &mut buf).unwrap();
    stdin.write_all(&buf).await.unwrap();
    stdin.flush().await.unwrap();
}

async fn recv_frame(
    stdout: &mut tokio::process::ChildStdout,
    read_buf: &mut BytesMut,
) -> v3::ServerEnvelope {
    loop {
        match decode_frame::<v3::ServerEnvelope>(read_buf) {
            Ok(msg) => return msg,
            Err(rttx_proto::FrameError::Incomplete) => {}
            Err(e) => panic!("decode error: {e}"),
        }
        let n = tokio::time::timeout(Duration::from_secs(10), stdout.read_buf(read_buf))
            .await
            .expect("timed out reading from stdout")
            .expect("read failed");
        assert!(n > 0, "unexpected EOF");
    }
}

async fn handshake(
    stdin: &mut tokio::process::ChildStdin,
    stdout: &mut tokio::process::ChildStdout,
    read_buf: &mut BytesMut,
) {
    use rttx_proto::v3_handshake;
    let hello = v3_handshake::build_client_hello(
        uuid::Uuid::new_v4(),
        "test-client",
        "0.0.0",
        v3_handshake::CORE_CAPABILITIES,
    );
    send_frame(stdin, &hello).await;
    // The handshake response is a ServerHello frame.
    loop {
        match decode_frame::<v3::ServerHello>(read_buf) {
            Ok(_) => return,
            Err(rttx_proto::FrameError::Incomplete) => {}
            Err(e) => panic!("decode ServerHello error: {e}"),
        }
        let n = tokio::time::timeout(Duration::from_secs(10), stdout.read_buf(read_buf))
            .await
            .expect("timed out reading ServerHello")
            .expect("read failed");
        assert!(n > 0, "unexpected EOF during handshake");
    }
}

async fn wait_for_exit(child: &mut Child) -> std::process::ExitStatus {
    tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("timed out waiting for process exit")
        .expect("wait failed")
}

// ── Stdin close (peer disconnect) exits cleanly ─────────────────

#[tokio::test]
async fn stdin_close_before_handshake_exits_cleanly() {
    let tmp = TempDir::new().unwrap();
    let mut _daemon = spawn_daemon(&tmp).await;
    let mut child = spawn_stdio(&tmp);
    let stdin = child.stdin.take().unwrap();

    // Close stdin immediately without sending anything.
    drop(stdin);

    let status = wait_for_exit(&mut child).await;
    assert!(status.success(), "process must exit cleanly on stdin close: {status}");
}

#[tokio::test]
async fn stdin_close_after_handshake_exits_cleanly() {
    let tmp = TempDir::new().unwrap();
    let mut _daemon = spawn_daemon(&tmp).await;
    let mut child = spawn_stdio(&tmp);
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut read_buf = BytesMut::with_capacity(4096);

    handshake(&mut stdin, &mut stdout, &mut read_buf).await;

    // Close stdin — simulates SSH disconnect.
    drop(stdin);

    let status = wait_for_exit(&mut child).await;
    assert!(status.success(), "process must exit cleanly after handshake disconnect: {status}");
}

#[tokio::test]
async fn stdin_close_after_session_create_exits_cleanly() {
    let tmp = TempDir::new().unwrap();
    let mut _daemon = spawn_daemon(&tmp).await;
    let mut child = spawn_stdio(&tmp);
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut read_buf = BytesMut::with_capacity(4096);

    handshake(&mut stdin, &mut stdout, &mut read_buf).await;

    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateRuntime(v3::CreateRuntime {
            name: "disconnect-test".into(),
            policy: v3::RuntimePolicy::Persistent as i32,
        })),
    };
    send_frame(&mut stdin, &create).await;
    let resp = recv_frame(&mut stdout, &mut read_buf).await;
    assert!(matches!(resp.payload, Some(v3::server_envelope::Payload::RuntimeCreated(_))));

    // Disconnect mid-session.
    drop(stdin);

    let status = wait_for_exit(&mut child).await;
    assert!(status.success(), "process must exit cleanly after session disconnect: {status}");
}

// ── Handshake mismatch ──────────────────────────────────────────

#[tokio::test]
async fn wrong_protocol_version_returns_error_over_stdio() {
    let tmp = TempDir::new().unwrap();
    let mut _daemon = spawn_daemon(&tmp).await;
    let mut child = spawn_stdio(&tmp);
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut read_buf = BytesMut::with_capacity(4096);

    let hello = v3::ClientHello {
        min_protocol_version: 9999,
        max_protocol_version: 9999,
        client_id: uuid_to_bytes(uuid::Uuid::new_v4()),
        client_name: "test-client".into(),
        client_version: "0.0.0".into(),
        capabilities: vec![],
    };
    send_frame(&mut stdin, &hello).await;

    // On version mismatch the server sends a bare ProtocolError frame.
    let resp = loop {
        match decode_frame::<v3::ProtocolError>(&mut read_buf) {
            Ok(err) => break err,
            Err(rttx_proto::FrameError::Incomplete) => {}
            Err(e) => panic!("decode ProtocolError error: {e}"),
        }
        let n = tokio::time::timeout(Duration::from_secs(10), stdout.read_buf(&mut read_buf))
            .await
            .expect("timed out reading ProtocolError")
            .expect("read failed");
        assert!(n > 0, "unexpected EOF waiting for version mismatch error");
    };
    assert_eq!(resp.kind, v3::ErrorKind::ProtocolMismatch as i32);

    drop(stdin);
    let status = wait_for_exit(&mut child).await;
    assert!(status.success(), "process must exit cleanly after version mismatch: {status}");
}

// ── Garbage input ───────────────────────────────────────────────

#[tokio::test]
async fn garbage_bytes_on_stdin_exits_without_hang() {
    let tmp = TempDir::new().unwrap();
    let mut _daemon = spawn_daemon(&tmp).await;
    let mut child = spawn_stdio(&tmp);
    let mut stdin = child.stdin.take().unwrap();

    // Send random garbage, not a valid protobuf frame.
    stdin.write_all(b"this is not a protocol frame").await.unwrap();
    let _ = stdin.flush().await;
    drop(stdin);

    let status = wait_for_exit(&mut child).await;
    // Process may exit with error code, but must not hang.
    assert!(status.code().is_some(), "process must exit (not hang) on garbage input");
}

#[tokio::test]
async fn truncated_frame_on_stdin_exits_without_hang() {
    let tmp = TempDir::new().unwrap();
    let mut _daemon = spawn_daemon(&tmp).await;
    let mut child = spawn_stdio(&tmp);
    let mut stdin = child.stdin.take().unwrap();

    // Write a length prefix claiming 1000 bytes, then close stdin.
    let len: u32 = 1000;
    stdin.write_all(&len.to_le_bytes()).await.unwrap();
    stdin.write_all(&[0u8; 4]).await.unwrap();
    let _ = stdin.flush().await;
    drop(stdin);

    let status = wait_for_exit(&mut child).await;
    assert!(status.code().is_some(), "process must exit (not hang) on truncated frame");
}

// ── Empty message body ──────────────────────────────────────────

#[tokio::test]
async fn empty_message_returns_error_over_stdio() {
    let tmp = TempDir::new().unwrap();
    let mut _daemon = spawn_daemon(&tmp).await;
    let mut child = spawn_stdio(&tmp);
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut read_buf = BytesMut::with_capacity(4096);

    handshake(&mut stdin, &mut stdout, &mut read_buf).await;

    let empty = v3::ClientEnvelope { request_id: 0, command: None };
    send_frame(&mut stdin, &empty).await;

    let resp = recv_frame(&mut stdout, &mut read_buf).await;
    match resp.payload {
        Some(v3::server_envelope::Payload::Error(e)) => {
            assert!(e.kind != 0, "expected error for empty message, got kind {}", e.kind);
        }
        other => panic!("expected Error, got {other:?}"),
    }

    drop(stdin);
    let status = wait_for_exit(&mut child).await;
    assert!(status.success(), "process must exit cleanly: {status}");
}
