//! Integration test for the attach-stdio transport.
//!
//! Verifies that the protocol works over a pipe (simulating SSH stdio)
//! by spawning the server binary with `attach-stdio` and communicating
//! over its stdin/stdout.

use bytes::BytesMut;
use rttx_proto::{
    PROTOCOL_VERSION, bytes_to_uuid, decode_frame, encode_frame, proto, uuid_to_bytes,
};
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

/// Spawn `rttx-server attach-stdio` and speak the protocol over pipes.
#[tokio::test]
async fn attach_stdio_hello_and_create_session() {
    let bin = env!("CARGO_BIN_EXE_rttx-server");

    let mut child = Command::new(bin)
        .arg("attach-stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn rttx-server attach-stdio");

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();

    // Send Hello.
    let hello = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Hello(proto::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_id: uuid_to_bytes(uuid::Uuid::new_v4()),
        })),
    };
    let mut buf = BytesMut::new();
    encode_frame(&hello, &mut buf).unwrap();
    stdin.write_all(&buf).await.unwrap();
    stdin.flush().await.unwrap();

    // Read HelloAck.
    let mut read_buf = BytesMut::with_capacity(4096);
    let ack: proto::ServerMessage = loop {
        let n = stdout.read_buf(&mut read_buf).await.unwrap();
        assert!(n > 0, "unexpected EOF waiting for HelloAck");
        match decode_frame::<proto::ServerMessage>(&mut read_buf) {
            Ok(msg) => break msg,
            Err(rttx_proto::FrameError::Incomplete) => {}
            Err(e) => panic!("decode error: {e}"),
        }
    };
    assert!(
        matches!(ack.msg, Some(proto::server_message::Msg::HelloAck(_))),
        "expected HelloAck, got {ack:?}"
    );

    // Create a session.
    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateSession(proto::CreateSession {
            name: "stdio-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    buf.clear();
    encode_frame(&create, &mut buf).unwrap();
    stdin.write_all(&buf).await.unwrap();
    stdin.flush().await.unwrap();

    // Read SessionCreated.
    let resp: proto::ServerMessage = loop {
        let n = stdout.read_buf(&mut read_buf).await.unwrap();
        assert!(n > 0, "unexpected EOF waiting for SessionCreated");
        match decode_frame::<proto::ServerMessage>(&mut read_buf) {
            Ok(msg) => break msg,
            Err(rttx_proto::FrameError::Incomplete) => {}
            Err(e) => panic!("decode error: {e}"),
        }
    };
    let session_id = match resp.msg {
        Some(proto::server_message::Msg::SessionCreated(sc)) => {
            bytes_to_uuid(&sc.session_id).unwrap()
        }
        other => panic!("expected SessionCreated, got {other:?}"),
    };
    assert!(!session_id.is_nil());

    // List sessions — should have one.
    let list = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
    };
    buf.clear();
    encode_frame(&list, &mut buf).unwrap();
    stdin.write_all(&buf).await.unwrap();
    stdin.flush().await.unwrap();

    let resp: proto::ServerMessage = loop {
        let n = stdout.read_buf(&mut read_buf).await.unwrap();
        assert!(n > 0, "unexpected EOF waiting for SessionList");
        match decode_frame::<proto::ServerMessage>(&mut read_buf) {
            Ok(msg) => break msg,
            Err(rttx_proto::FrameError::Incomplete) => {}
            Err(e) => panic!("decode error: {e}"),
        }
    };
    match resp.msg {
        Some(proto::server_message::Msg::SessionList(sl)) => {
            assert_eq!(sl.sessions.len(), 1);
            assert_eq!(sl.sessions[0].name, "stdio-test");
        }
        other => panic!("expected SessionList, got {other:?}"),
    }

    // Close stdin to disconnect — the process should exit.
    drop(stdin);
    let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
        .await
        .expect("timed out waiting for process exit")
        .expect("wait failed");
    assert!(status.success() || status.code() == Some(0), "unexpected exit: {status}");
}
