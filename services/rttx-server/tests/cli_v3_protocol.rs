//! Integration tests verifying CLI commands use v3 protocol exclusively.
//!
//! These tests exercise the same v3 handshake + envelope pattern that the
//! CLI commands (`stop`, `status`, `diagnostics`, `clean`, `kill`) use.

mod common;

use bytes::BytesMut;
use common::start_test_server;
use rttx_proto::{decode_frame, encode_frame, v3};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// Perform a v3 handshake on a raw stream (same pattern as CLI `v3_connect`).
async fn cli_v3_handshake(stream: &mut UnixStream) {
    let hello = rttx_proto::v3_handshake::build_client_hello(
        uuid::Uuid::new_v4(),
        "rttx-server-cli",
        "0.0.0-test",
        &[
            v3::Capability::CoreRuntimeLifecycle,
            v3::Capability::CorePaneLifecycle,
            v3::Capability::CoreTerminalIo,
            v3::Capability::CoreTerminalModes,
            v3::Capability::CorePasteIntent,
            v3::Capability::CoreFocusEvents,
            v3::Capability::OptDiagnostics,
        ],
    );
    let mut buf = BytesMut::new();
    encode_frame(&hello, &mut buf).unwrap();
    stream.write_all(&buf).await.unwrap();
    stream.flush().await.unwrap();

    // Read ServerHello.
    let mut read_buf = BytesMut::with_capacity(4096);
    loop {
        stream.read_buf(&mut read_buf).await.unwrap();
        match decode_frame::<v3::ServerHello>(&mut read_buf) {
            Ok(_) => break,
            Err(rttx_proto::FrameError::Incomplete) => {}
            Err(e) => panic!("failed to decode ServerHello: {e}"),
        }
    }
}

/// Send a v3 envelope and receive the response.
async fn send_and_recv(stream: &mut UnixStream, env: &v3::ClientEnvelope) -> v3::ServerEnvelope {
    let mut buf = BytesMut::new();
    encode_frame(env, &mut buf).unwrap();
    stream.write_all(&buf).await.unwrap();
    stream.flush().await.unwrap();

    let mut read_buf = BytesMut::with_capacity(8192);
    loop {
        stream.read_buf(&mut read_buf).await.unwrap();
        match decode_frame::<v3::ServerEnvelope>(&mut read_buf) {
            Ok(resp) => return resp,
            Err(rttx_proto::FrameError::Incomplete) => {}
            Err(e) => panic!("decode error: {e}"),
        }
    }
}

#[tokio::test]
async fn cli_status_via_v3_list_runtimes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut stream = UnixStream::connect(&sock).await.unwrap();
    cli_v3_handshake(&mut stream).await;

    let id_gen = rttx_proto::v3_envelope::RequestIdGenerator::new();
    let env = rttx_proto::v3_envelope::build_client_envelope(
        &id_gen,
        v3::client_envelope::Command::ListRuntimes(v3::ListRuntimes {}),
    );

    let resp = send_and_recv(&mut stream, &env).await;
    assert!(
        matches!(resp.payload, Some(v3::server_envelope::Payload::RuntimeList(_))),
        "expected RuntimeList, got {:?}",
        resp.payload
    );
}

#[tokio::test]
async fn cli_diagnostics_via_v3() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut stream = UnixStream::connect(&sock).await.unwrap();
    cli_v3_handshake(&mut stream).await;

    let id_gen = rttx_proto::v3_envelope::RequestIdGenerator::new();
    let env = rttx_proto::v3_envelope::build_client_envelope(
        &id_gen,
        v3::client_envelope::Command::GetDiagnostics(v3::GetDiagnostics {}),
    );

    let resp = send_and_recv(&mut stream, &env).await;
    match resp.payload {
        Some(v3::server_envelope::Payload::DiagnosticsReport(report)) => {
            assert_eq!(report.runtime_count, 0);
            assert_eq!(report.total_pane_count, 0);
            assert_eq!(report.client_count, 1); // this test client
        }
        other => panic!("expected DiagnosticsReport, got {other:?}"),
    }
}

#[tokio::test]
async fn cli_kill_via_v3_terminate_runtime() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    // Create a runtime via v3 first.
    let mut v3_client = common::TestV3Client::connect(&sock).await;
    let runtime_id = v3_client.create_runtime("kill-target").await;

    // Now simulate the CLI kill command via a fresh v3 connection.
    let mut stream = UnixStream::connect(&sock).await.unwrap();
    cli_v3_handshake(&mut stream).await;

    let id_gen = rttx_proto::v3_envelope::RequestIdGenerator::new();
    let env = rttx_proto::v3_envelope::build_client_envelope(
        &id_gen,
        v3::client_envelope::Command::TerminateRuntime(v3::TerminateRuntime {
            runtime_id: runtime_id.clone(),
        }),
    );

    let resp = send_and_recv(&mut stream, &env).await;
    assert!(
        matches!(resp.payload, Some(v3::server_envelope::Payload::RuntimeTerminated(_))),
        "expected RuntimeTerminated, got {:?}",
        resp.payload
    );
}

#[tokio::test]
async fn cli_kill_nonexistent_returns_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut stream = UnixStream::connect(&sock).await.unwrap();
    cli_v3_handshake(&mut stream).await;

    let id_gen = rttx_proto::v3_envelope::RequestIdGenerator::new();
    let env = rttx_proto::v3_envelope::build_client_envelope(
        &id_gen,
        v3::client_envelope::Command::TerminateRuntime(v3::TerminateRuntime {
            runtime_id: rttx_proto::uuid_to_bytes(uuid::Uuid::new_v4()),
        }),
    );

    let resp = send_and_recv(&mut stream, &env).await;
    match resp.payload {
        Some(v3::server_envelope::Payload::Error(e)) => {
            assert_eq!(e.kind, v3::ErrorKind::RuntimeNotFound as i32);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[tokio::test]
async fn cli_clean_via_v3_list_and_terminate() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    // Create a runtime (no clients attached after creation without attach).
    let mut v3_client = common::TestV3Client::connect(&sock).await;
    let runtime_id = v3_client.create_runtime("clean-target").await;
    drop(v3_client);

    // Small delay for disconnect to propagate.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Simulate CLI clean: list runtimes, find unused, terminate.
    let mut stream = UnixStream::connect(&sock).await.unwrap();
    cli_v3_handshake(&mut stream).await;

    let id_gen = rttx_proto::v3_envelope::RequestIdGenerator::new();

    // List runtimes.
    let list_env = rttx_proto::v3_envelope::build_client_envelope(
        &id_gen,
        v3::client_envelope::Command::ListRuntimes(v3::ListRuntimes {}),
    );
    let resp = send_and_recv(&mut stream, &list_env).await;
    let runtimes = match resp.payload {
        Some(v3::server_envelope::Payload::RuntimeList(sl)) => sl.runtimes,
        other => panic!("expected RuntimeList, got {other:?}"),
    };

    // Find unused (no write owner, no readers).
    let unused: Vec<_> =
        runtimes.iter().filter(|r| !r.has_write_owner && r.read_only_client_count == 0).collect();
    assert_eq!(unused.len(), 1);
    assert_eq!(unused[0].id, runtime_id);

    // Terminate unused.
    let term_env = rttx_proto::v3_envelope::build_client_envelope(
        &id_gen,
        v3::client_envelope::Command::TerminateRuntime(v3::TerminateRuntime {
            runtime_id: runtime_id.clone(),
        }),
    );
    let resp = send_and_recv(&mut stream, &term_env).await;
    assert!(matches!(resp.payload, Some(v3::server_envelope::Payload::RuntimeTerminated(_))));

    // Verify it's gone.
    let resp = send_and_recv(&mut stream, &list_env).await;
    match resp.payload {
        Some(v3::server_envelope::Payload::RuntimeList(sl)) => {
            assert!(sl.runtimes.is_empty());
        }
        other => panic!("expected empty RuntimeList, got {other:?}"),
    }
}

#[tokio::test]
async fn cli_stop_via_v3_shutdown() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, handle) = start_test_server(tmp.path()).await;

    let mut stream = UnixStream::connect(&sock).await.unwrap();
    cli_v3_handshake(&mut stream).await;

    let id_gen = rttx_proto::v3_envelope::RequestIdGenerator::new();
    let env = rttx_proto::v3_envelope::build_client_envelope(
        &id_gen,
        v3::client_envelope::Command::Shutdown(v3::Shutdown {}),
    );

    let mut buf = BytesMut::new();
    encode_frame(&env, &mut buf).unwrap();
    stream.write_all(&buf).await.unwrap();
    stream.flush().await.unwrap();

    // Server should shut down.
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    assert!(result.is_ok(), "server should shut down after Shutdown command");
}
