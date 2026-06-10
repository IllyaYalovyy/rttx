//! Integration tests for the `rttx-server kill <runtime-id>` command.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::v3;

#[tokio::test]
async fn kill_terminates_existing_runtime() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let runtime_id =
        common::create_runtime(&mut client, "target", v3::RuntimePolicy::Persistent).await;

    // Verify it exists.
    let runtimes = common::list_runtimes(&mut client).await;
    assert_eq!(runtimes.len(), 1);

    // Terminate via the same protocol path the kill command uses.
    common::terminate_runtime(&mut client, &runtime_id).await;

    // Verify it's gone.
    let runtimes = common::list_runtimes(&mut client).await;
    assert!(runtimes.is_empty());
}

#[tokio::test]
async fn kill_nonexistent_runtime_returns_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let fake_id = rttx_proto::uuid_to_bytes(uuid::Uuid::new_v4());

    let msg = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::TerminateRuntime(v3::TerminateRuntime {
            runtime_id: fake_id,
        })),
    };
    client.send(&msg).await;

    let resp = client.recv().await;
    match resp.payload {
        Some(v3::server_envelope::Payload::Error(e)) => {
            assert!(e.message.contains("not found"), "expected 'not found', got: {}", e.message);
        }
        other => panic!("expected Error, got: {other:?}"),
    }
}

/// Binary-level test: `rttx-server kill` with invalid UUID exits with error.
#[test]
fn kill_invalid_uuid_exits_with_error() {
    let bin = env!("CARGO_BIN_EXE_rttx-server");
    let tmp = tempfile::TempDir::new().unwrap();
    let runtime_dir = tmp.path().join("runtime");
    std::fs::create_dir_all(&runtime_dir).unwrap();

    let output = std::process::Command::new(bin)
        .args(["kill", "not-a-uuid"])
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("RTTX_DEV_MODE", "")
        .output()
        .expect("failed to run kill");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid runtime ID"),
        "expected 'invalid runtime ID' in stderr, got: {stderr}"
    );
}

/// Binary-level test: `rttx-server kill` reports daemon not running.
#[test]
fn kill_reports_not_running_without_daemon() {
    let bin = env!("CARGO_BIN_EXE_rttx-server");
    let tmp = tempfile::TempDir::new().unwrap();
    let runtime_dir = tmp.path().join("runtime");
    std::fs::create_dir_all(&runtime_dir).unwrap();

    let output = std::process::Command::new(bin)
        .args(["kill", "d7d04564-b2bf-4302-9495-e65c4df12ac6"])
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("RTTX_DEV_MODE", "")
        .output()
        .expect("failed to run kill");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not running"), "should report daemon not running, got: {stderr}");
}
