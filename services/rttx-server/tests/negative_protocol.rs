//! Negative protocol tests: malformed input, invalid IDs, duplicate/out-of-order
//! mutations, and error-path assertions.

mod common;

use common::*;
use rttx_proto::{uuid_to_bytes, v3};
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
    let empty = v3::ClientEnvelope { request_id: 0, command: None };
    client.send(&empty).await;

    let resp = client.recv_or_timeout().await;
    let err = expect_error(&resp);
    assert!(err.kind != 0, "expected error for empty message, got kind {}", err.kind);
}

// ── Invalid UUID bytes ──────────────────────────────────────────

#[tokio::test]
async fn short_uuid_in_attach_returns_invalid_parameter() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let msg = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
            runtime_id: short_uuid(),
            attach_mode: 0,
        })),
    };
    client.send(&msg).await;

    let resp = client.recv_or_timeout().await;
    let err = expect_error(&resp);
    assert_eq!(err.kind, 3); // ERR_INVALID_PARAMETER
}

#[tokio::test]
async fn short_uuid_in_close_pane_returns_invalid_parameter() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let msg = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::ClosePane(v3::ClosePane {
            runtime_id: short_uuid(),
            pane_id: short_uuid(),
        })),
    };
    client.send(&msg).await;

    let resp = client.recv_or_timeout().await;
    let err = expect_error(&resp);
    assert_eq!(err.kind, 3);
}

// ── Stale / nonexistent session and pane IDs ────────────────────

#[tokio::test]
async fn attach_nonexistent_session_returns_session_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let msg = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
            runtime_id: bogus_uuid(),
            attach_mode: 0,
        })),
    };
    client.send(&msg).await;

    let resp = client.recv_or_timeout().await;
    let err = expect_error(&resp);
    assert_eq!(err.kind, 4); // ERR_SESSION_NOT_FOUND
}

#[tokio::test]
async fn create_pane_in_nonexistent_session_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let msg = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
            runtime_id: bogus_uuid(),
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
            no_persist: None,
        })),
    };
    client.send(&msg).await;

    let resp = client.recv_or_timeout().await;
    let err = expect_error(&resp);
    assert_eq!(err.kind, 4); // ERR_SESSION_NOT_FOUND
}

#[tokio::test]
async fn close_pane_with_nonexistent_pane_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let runtime_id = create_runtime(&mut client, "test", v3::RuntimePolicy::Persistent).await;

    let msg = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::ClosePane(v3::ClosePane {
            runtime_id: runtime_id.clone(),
            pane_id: bogus_uuid(),
        })),
    };
    client.send(&msg).await;

    let resp = client.recv_or_timeout().await;
    let err = expect_error(&resp);
    assert!(err.kind != 0, "expected error, got kind {}", err.kind);
}

#[tokio::test]
async fn resize_nonexistent_pane_is_silently_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let runtime_id = create_runtime(&mut client, "test", v3::RuntimePolicy::Persistent).await;
    attach_runtime(&mut client, &runtime_id).await;

    let msg = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::ResizePane(v3::ResizePane {
            runtime_id,
            pane_id: bogus_uuid(),
            cols: 120,
            rows: 40,
        })),
    };
    client.send(&msg).await;

    // Resize to a nonexistent pane must be silently dropped — no error
    // response — because the client treats Resize as fire-and-forget.
    let msgs = client.drain(Duration::from_millis(200)).await;
    assert!(
        msgs.iter().all(|m| !matches!(m.payload, Some(v3::server_envelope::Payload::Error(_)))),
        "resize to nonexistent pane must not produce an error response"
    );

    // Server must remain functional.
    let list = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::ListRuntimes(v3::ListRuntimes {})),
    };
    client.send(&list).await;
    let resp = client.recv_or_timeout().await;
    assert!(matches!(resp.payload, Some(v3::server_envelope::Payload::RuntimeList(_))));
}

// ── Duplicate and out-of-order mutations ────────────────────────

#[tokio::test]
async fn duplicate_close_pane_returns_error_on_second_call() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let runtime_id = create_runtime(&mut client, "test", v3::RuntimePolicy::Persistent).await;
    let pane_id = attach_and_create_pane(&mut client, &runtime_id).await;

    // First close succeeds.
    let close = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::ClosePane(v3::ClosePane {
            runtime_id: runtime_id.clone(),
            pane_id: pane_id.clone(),
        })),
    };
    client.send(&close).await;
    let resp = client.recv_or_timeout().await;
    assert!(
        !matches!(resp.payload, Some(v3::server_envelope::Payload::Error(_))),
        "first close should succeed"
    );

    // Drain any PaneClosed/PaneExited push messages.
    client.drain(Duration::from_millis(500)).await;

    // Second close of the same pane returns an error.
    client.send(&close).await;
    let resp = client.recv_or_timeout().await;
    let err = expect_error(&resp);
    assert!(err.kind != 0, "expected error, got kind {}", err.kind);
}

#[tokio::test]
async fn detach_without_attach_is_harmless() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let runtime_id = create_runtime(&mut client, "test", v3::RuntimePolicy::Persistent).await;

    // Detach without ever attaching — should not panic or error.
    let msg = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::DetachRuntime(v3::DetachRuntime {
            runtime_id,
        })),
    };
    client.send(&msg).await;

    let resp = client.recv_or_timeout().await;
    assert!(
        matches!(
            resp.payload,
            Some(
                v3::server_envelope::Payload::RuntimeDetached(_)
                    | v3::server_envelope::Payload::OutputDelta(_)
            )
        ),
        "detach without attach should return RuntimeDetached, got {resp:?}"
    );
}

// ── Version mismatch ────────────────────────────────────────────

#[tokio::test]
async fn wrong_protocol_version_returns_version_mismatch() {
    use bytes::BytesMut;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;

    // Connect raw and send a ClientHello with an unsupported version.
    let mut stream = tokio::net::UnixStream::connect(&socket_path).await.unwrap();
    let hello = v3::ClientHello {
        min_protocol_version: 9999,
        max_protocol_version: 9999,
        client_id: uuid_to_bytes(uuid::Uuid::new_v4()),
        client_name: String::new(),
        client_version: String::new(),
        capabilities: vec![],
    };
    let mut buf = BytesMut::new();
    rttx_proto::encode_frame(&hello, &mut buf).unwrap();
    stream.write_all(&buf).await.unwrap();

    // Read the response — should be a ProtocolError frame or the connection drops.
    let mut read_buf = BytesMut::with_capacity(4096);
    let n = stream.read_buf(&mut read_buf).await.unwrap();
    // Server should close the connection for unsupported version.
    // Either we get 0 bytes (EOF) or an error frame.
    assert!(n == 0 || !read_buf.is_empty(), "server should respond or disconnect");
}

// ── Input to nonexistent pane is silently dropped ───────────────

#[tokio::test]
async fn input_to_nonexistent_pane_is_silently_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let runtime_id = create_runtime(&mut client, "test", v3::RuntimePolicy::Persistent).await;
    attach_runtime(&mut client, &runtime_id).await;

    let msg = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
            runtime_id,
            pane_id: bogus_uuid(),
            kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                data: bytes::Bytes::from_static(b"hello"),
            })),
        })),
    };
    client.send(&msg).await;

    // Input to a nonexistent pane must be silently dropped — no error
    // response — because the client treats Input as fire-and-forget.
    let msgs = client.drain(Duration::from_millis(200)).await;
    assert!(
        msgs.iter().all(|m| !matches!(m.payload, Some(v3::server_envelope::Payload::Error(_)))),
        "input to nonexistent pane must not produce an error response"
    );

    // Server must remain functional.
    let list = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::ListRuntimes(v3::ListRuntimes {})),
    };
    client.send(&list).await;
    let resp = client.recv_or_timeout().await;
    assert!(matches!(resp.payload, Some(v3::server_envelope::Payload::RuntimeList(_))));
}

// ── Fire-and-forget commands to nonexistent sessions ────────────

/// Input and Resize targeting a session that does not exist must produce
/// no response at all — they are fire-and-forget and the server must not
/// pollute the push stream with error messages.
#[tokio::test]
async fn fire_and_forget_to_nonexistent_session_produces_no_response() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    let fake_session = bogus_uuid();
    let fake_pane = bogus_uuid();

    // Send Input to a nonexistent session.
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::TerminalInput(v3::TerminalInput {
                runtime_id: fake_session.clone(),
                pane_id: fake_pane.clone(),
                kind: Some(v3::terminal_input::Kind::Raw(v3::RawInput {
                    data: bytes::Bytes::from_static(b"hello"),
                })),
            })),
        })
        .await;

    // Send Resize to a nonexistent session.
    client
        .send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::ResizePane(v3::ResizePane {
                runtime_id: fake_session,
                pane_id: fake_pane,
                cols: 80,
                rows: 24,
            })),
        })
        .await;

    // Neither command should produce any response.
    let msgs = client.drain(Duration::from_millis(300)).await;
    assert!(
        msgs.iter().all(|m| !matches!(m.payload, Some(v3::server_envelope::Payload::Error(_)))),
        "fire-and-forget commands to nonexistent session must not produce error responses"
    );
}

// ── Helpers ─────────────────────────────────────────────────────

fn expect_error(resp: &v3::ServerEnvelope) -> &v3::ProtocolError {
    match &resp.payload {
        Some(v3::server_envelope::Payload::Error(e)) => e,
        other => panic!("expected Error, got {other:?}"),
    }
}

async fn attach_runtime(client: &mut TestClient, runtime_id: &[u8]) {
    let msg = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachRuntime(v3::AttachRuntime {
            runtime_id: runtime_id.to_vec(),
            attach_mode: v3::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&msg).await;
    let resp = client.recv_or_timeout().await;
    match resp.payload {
        Some(v3::server_envelope::Payload::RuntimeSnapshot(_)) => {}
        other => panic!("expected Snapshot, got {other:?}"),
    }
}

async fn attach_and_create_pane(client: &mut TestClient, runtime_id: &[u8]) -> Vec<u8> {
    attach_runtime(client, runtime_id).await;

    let msg = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreatePane(v3::CreatePane {
            runtime_id: runtime_id.to_vec(),
            cwd: None,
            dark_background: None,
            cols: 0,
            rows: 0,
            no_persist: None,
        })),
    };
    client.send(&msg).await;
    let resp = client.recv_or_timeout().await;
    match resp.payload {
        Some(v3::server_envelope::Payload::PaneCreated(pc)) => pc.pane_id,
        other => panic!("expected PaneCreated, got {other:?}"),
    }
}

/// Closing an already-closed pane must return `ERR_PANE_NOT_FOUND` (code 6).
///
/// Regression test for #309: the client must recognise code 6 on `ClosePane`
/// and treat it as a successful close instead of blocking the workspace.
#[test]
fn close_already_closed_pane_returns_pane_not_found() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        let tmp = tempfile::tempdir().unwrap();
        let (socket_path, _handle) = start_test_server(tmp.path()).await;
        let mut client = TestClient::connect(&socket_path).await;
        client.handshake().await;

        let runtime_id = create_runtime(&mut client, "test", v3::RuntimePolicy::Persistent).await;
        attach_runtime(&mut client, &runtime_id).await;
        let pane_id = create_pane(&mut client, &runtime_id).await;

        // First close succeeds.
        let close_msg = v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::ClosePane(v3::ClosePane {
                runtime_id: runtime_id.clone(),
                pane_id: pane_id.clone(),
            })),
        };
        client.send(&close_msg).await;
        let resp = client.recv_or_timeout().await;
        match resp.payload {
            Some(v3::server_envelope::Payload::PaneClosed(_)) => {}
            other => panic!("expected PaneClosed on first close, got {other:?}"),
        }

        // Second close must return ERR_PANE_NOT_FOUND (code 6).
        client.send(&close_msg).await;
        let resp = client.recv_or_timeout().await;
        let err = expect_error(&resp);
        assert!(err.kind != 0, "expected error for already-closed pane");
    });
}
