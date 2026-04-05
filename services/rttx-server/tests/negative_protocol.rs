//! Negative protocol tests: malformed input, invalid IDs, duplicate/out-of-order
//! mutations, and error-path assertions.

mod common;

use common::*;
use rttx_proto::{proto, uuid_to_bytes};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

fn bogus_uuid() -> Vec<u8> {
    uuid_to_bytes(uuid::Uuid::nil())
}

fn short_uuid() -> Vec<u8> {
    vec![0u8; 4]
}

// ── Truncated and oversized frames ──────────────────────────────

#[tokio::test]
async fn truncated_frame_disconnects_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;

    let mut stream = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
    // Write a length prefix claiming 100 bytes, then only send 4 bytes of payload.
    let len: u32 = 100;
    stream.write_all(&len.to_le_bytes()).await.unwrap();
    stream.write_all(&[0u8; 4]).await.unwrap();
    drop(stream);
    // Server must not panic — it just drops the connection.
}

#[tokio::test]
async fn oversized_frame_length_disconnects_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;

    let mut stream = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
    // Claim a frame larger than MAX_MESSAGE_SIZE.
    let len: u32 = rttx_proto::MAX_MESSAGE_SIZE + 1;
    stream.write_all(&len.to_le_bytes()).await.unwrap();
    stream.write_all(&[0u8; 64]).await.unwrap();
    drop(stream);
}

#[tokio::test]
async fn garbage_bytes_disconnect_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;

    let mut stream = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
    stream.write_all(b"this is not a protobuf frame at all").await.unwrap();
    drop(stream);
}

// ── Empty and missing message body ──────────────────────────────

#[tokio::test]
async fn empty_message_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    // Send a ClientMessage with msg = None.
    let empty = proto::ClientMessage { msg: None };
    client.send(&empty).await;

    let resp = client.recv_or_timeout().await;
    let err = expect_error(&resp);
    assert_eq!(err.code, 1); // ERR_EMPTY_MESSAGE
}

// ── Invalid UUID bytes ──────────────────────────────────────────

#[tokio::test]
async fn short_uuid_in_attach_returns_invalid_parameter() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
            session_id: short_uuid(),
            attach_mode: 0,
        })),
    };
    client.send(&msg).await;

    let resp = client.recv_or_timeout().await;
    let err = expect_error(&resp);
    assert_eq!(err.code, 3); // ERR_INVALID_PARAMETER
}

#[tokio::test]
async fn short_uuid_in_close_pane_returns_invalid_parameter() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
            session_id: short_uuid(),
            pane_id: short_uuid(),
        })),
    };
    client.send(&msg).await;

    let resp = client.recv_or_timeout().await;
    let err = expect_error(&resp);
    assert_eq!(err.code, 3);
}

// ── Stale / nonexistent session and pane IDs ────────────────────

#[tokio::test]
async fn attach_nonexistent_session_returns_session_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
            session_id: bogus_uuid(),
            attach_mode: 0,
        })),
    };
    client.send(&msg).await;

    let resp = client.recv_or_timeout().await;
    let err = expect_error(&resp);
    assert_eq!(err.code, 4); // ERR_SESSION_NOT_FOUND
}

#[tokio::test]
async fn create_pane_in_nonexistent_session_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
            session_id: bogus_uuid(),
        })),
    };
    client.send(&msg).await;

    let resp = client.recv_or_timeout().await;
    let err = expect_error(&resp);
    assert_eq!(err.code, 4); // ERR_SESSION_NOT_FOUND
}

#[tokio::test]
async fn close_pane_with_nonexistent_pane_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let session_id = create_session(&mut client, "test", proto::RuntimePolicy::Persistent).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
            session_id: session_id.clone(),
            pane_id: bogus_uuid(),
        })),
    };
    client.send(&msg).await;

    let resp = client.recv_or_timeout().await;
    let err = expect_error(&resp);
    assert!(err.code == 6 || err.code == 4); // ERR_PANE_NOT_FOUND or ERR_SESSION_NOT_FOUND
}

#[tokio::test]
async fn resize_nonexistent_pane_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let session_id = create_session(&mut client, "test", proto::RuntimePolicy::Persistent).await;
    attach_session(&mut client, &session_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Resize(proto::Resize {
            session_id,
            pane_id: bogus_uuid(),
            cols: 120,
            rows: 40,
        })),
    };
    client.send(&msg).await;

    let resp = client.recv_or_timeout().await;
    let err = expect_error(&resp);
    assert!(err.code == 6 || err.code == 7); // ERR_PANE_NOT_FOUND or ERR_PANE_NOT_RUNNING
}

// ── Duplicate and out-of-order mutations ────────────────────────

#[tokio::test]
async fn duplicate_close_pane_returns_error_on_second_call() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let session_id = create_session(&mut client, "test", proto::RuntimePolicy::Persistent).await;
    let pane_id = attach_and_create_pane(&mut client, &session_id).await;

    // First close succeeds.
    let close = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ClosePane(proto::ClosePane {
            session_id: session_id.clone(),
            pane_id: pane_id.clone(),
        })),
    };
    client.send(&close).await;
    let resp = client.recv_or_timeout().await;
    assert!(
        !matches!(resp.msg, Some(proto::server_message::Msg::Error(_))),
        "first close should succeed"
    );

    // Drain any PaneClosed/PaneExited push messages.
    client.drain(Duration::from_millis(500)).await;

    // Second close of the same pane returns an error.
    client.send(&close).await;
    let resp = client.recv_or_timeout().await;
    let err = expect_error(&resp);
    assert!(err.code == 6 || err.code == 4); // ERR_PANE_NOT_FOUND
}

#[tokio::test]
async fn detach_without_attach_is_harmless() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let session_id = create_session(&mut client, "test", proto::RuntimePolicy::Persistent).await;

    // Detach without ever attaching — should not panic or error.
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::DetachSession(proto::DetachSession { session_id })),
    };
    client.send(&msg).await;

    let resp = client.recv_or_timeout().await;
    assert!(
        matches!(
            resp.msg,
            Some(
                proto::server_message::Msg::SessionDetached(_)
                    | proto::server_message::Msg::Delta(_)
            )
        ),
        "detach without attach should return SessionDetached, got {resp:?}"
    );
}

// ── Version mismatch ────────────────────────────────────────────

#[tokio::test]
async fn wrong_protocol_version_returns_version_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;

    let hello = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Hello(proto::Hello {
            protocol_version: 9999,
            client_id: uuid_to_bytes(uuid::Uuid::new_v4()),
        })),
    };
    client.send(&hello).await;

    let resp = client.recv_or_timeout().await;
    let err = expect_error(&resp);
    assert_eq!(err.code, 2); // ERR_VERSION_MISMATCH
}

// ── Input to nonexistent pane is silently dropped ───────────────

#[tokio::test]
async fn input_to_nonexistent_pane_does_not_crash() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let session_id = create_session(&mut client, "test", proto::RuntimePolicy::Persistent).await;
    attach_session(&mut client, &session_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Input(proto::Input {
            session_id,
            pane_id: bogus_uuid(),
            data: b"hello".to_vec(),
        })),
    };
    client.send(&msg).await;

    // Input to a nonexistent pane may return an error or be silently dropped.
    // Either way, the server must remain functional.
    // Drain any error response, then verify the server is still alive.
    client.drain(Duration::from_millis(200)).await;

    let list = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ListSessions(proto::ListSessions {})),
    };
    client.send(&list).await;
    let resp = client.recv_or_timeout().await;
    assert!(matches!(resp.msg, Some(proto::server_message::Msg::SessionList(_))));
}

// ── Helpers ─────────────────────────────────────────────────────

fn expect_error(resp: &proto::ServerMessage) -> &proto::Error {
    match &resp.msg {
        Some(proto::server_message::Msg::Error(e)) => e,
        other => panic!("expected Error, got {other:?}"),
    }
}

async fn attach_session(client: &mut TestClient, session_id: &[u8]) {
    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachSession(proto::AttachSession {
            session_id: session_id.to_vec(),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&msg).await;
    let resp = client.recv_or_timeout().await;
    match resp.msg {
        Some(proto::server_message::Msg::Snapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }
}

async fn attach_and_create_pane(client: &mut TestClient, session_id: &[u8]) -> Vec<u8> {
    attach_session(client, session_id).await;

    let msg = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
            session_id: session_id.to_vec(),
        })),
    };
    client.send(&msg).await;
    let resp = client.recv_or_timeout().await;
    match resp.msg {
        Some(proto::server_message::Msg::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    }
}
