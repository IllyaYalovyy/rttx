//! Attach-stdio failure-path tests.
//!
//! Exercises failure modes of the stdio transport: peer disconnect,
//! broken pipe, handshake mismatch, and garbage input.

use bytes::BytesMut;
use rttx_proto::{PROTOCOL_VERSION, decode_frame, encode_frame, proto, uuid_to_bytes};
use std::process::Stdio;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

async fn spawn_stdio(tmp: &TempDir) -> Child {
    let bin = env!("CARGO_BIN_EXE_rttx-server");
    let runtime_dir = tmp.path().join("runtime");
    let cache_dir = tmp.path().join("cache");
    tokio::fs::create_dir_all(&runtime_dir).await.unwrap();
    tokio::fs::create_dir_all(&cache_dir).await.unwrap();

    Command::new(bin)
        .arg("attach-stdio")
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("XDG_CACHE_HOME", &cache_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn rttx-server attach-stdio")
}

async fn send_frame(stdin: &mut tokio::process::ChildStdin, msg: &proto::ClientMessage) {
    let mut buf = BytesMut::new();
    encode_frame(msg, &mut buf).unwrap();
    stdin.write_all(&buf).await.unwrap();
    stdin.flush().await.unwrap();
}

async fn recv_frame(
    stdout: &mut tokio::process::ChildStdout,
    read_buf: &mut BytesMut,
) -> proto::ServerMessage {
    loop {
        match decode_frame::<proto::ServerMessage>(read_buf) {
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
    let hello = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Hello(proto::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_id: uuid_to_bytes(uuid::Uuid::new_v4()),
        })),
    };
    send_frame(stdin, &hello).await;
    let resp = recv_frame(stdout, read_buf).await;
    assert!(matches!(resp.msg, Some(proto::server_message::Msg::HelloAck(_))));
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
    let mut child = spawn_stdio(&tmp).await;
    let stdin = child.stdin.take().unwrap();

    // Close stdin immediately without sending anything.
    drop(stdin);

    let status = wait_for_exit(&mut child).await;
    assert!(status.success(), "process must exit cleanly on stdin close: {status}");
}

#[tokio::test]
async fn stdin_close_after_handshake_exits_cleanly() {
    let tmp = TempDir::new().unwrap();
    let mut child = spawn_stdio(&tmp).await;
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
    let mut child = spawn_stdio(&tmp).await;
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut read_buf = BytesMut::with_capacity(4096);

    handshake(&mut stdin, &mut stdout, &mut read_buf).await;

    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
            name: "disconnect-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    send_frame(&mut stdin, &create).await;
    let resp = recv_frame(&mut stdout, &mut read_buf).await;
    assert!(matches!(resp.msg, Some(proto::server_message::Msg::SessionCreated(_))));

    // Disconnect mid-session.
    drop(stdin);

    let status = wait_for_exit(&mut child).await;
    assert!(status.success(), "process must exit cleanly after session disconnect: {status}");
}

// ── Handshake mismatch ──────────────────────────────────────────

#[tokio::test]
async fn wrong_protocol_version_returns_error_over_stdio() {
    let tmp = TempDir::new().unwrap();
    let mut child = spawn_stdio(&tmp).await;
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut read_buf = BytesMut::with_capacity(4096);

    let hello = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Hello(proto::Hello {
            protocol_version: 9999,
            client_id: uuid_to_bytes(uuid::Uuid::new_v4()),
        })),
    };
    send_frame(&mut stdin, &hello).await;

    let resp = recv_frame(&mut stdout, &mut read_buf).await;
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, 2); // ERR_VERSION_MISMATCH
        }
        other => panic!("expected Error, got {other:?}"),
    }

    drop(stdin);
    let status = wait_for_exit(&mut child).await;
    assert!(status.success(), "process must exit cleanly after version mismatch: {status}");
}

// ── Garbage input ───────────────────────────────────────────────

#[tokio::test]
async fn garbage_bytes_on_stdin_exits_without_hang() {
    let tmp = TempDir::new().unwrap();
    let mut child = spawn_stdio(&tmp).await;
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
    let mut child = spawn_stdio(&tmp).await;
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
    let mut child = spawn_stdio(&tmp).await;
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut read_buf = BytesMut::with_capacity(4096);

    handshake(&mut stdin, &mut stdout, &mut read_buf).await;

    let empty = proto::ClientMessage { msg: None };
    send_frame(&mut stdin, &empty).await;

    let resp = recv_frame(&mut stdout, &mut read_buf).await;
    match resp.msg {
        Some(proto::server_message::Msg::Error(e)) => {
            assert_eq!(e.code, 1); // ERR_EMPTY_MESSAGE
        }
        other => panic!("expected Error, got {other:?}"),
    }

    drop(stdin);
    let status = wait_for_exit(&mut child).await;
    assert!(status.success(), "process must exit cleanly: {status}");
}
