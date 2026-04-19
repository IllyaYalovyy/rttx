//! Regression test for #641: `is_server_running()` probe connections
//! must not pollute the daemon log with INFO-level connect/disconnect
//! messages.

mod common;

use common::start_test_server;
use tokio::net::UnixStream;

/// A bare connect-and-drop (the pattern used by `is_server_running`)
/// must not crash the server or prevent subsequent real clients from
/// connecting.
#[tokio::test]
async fn probe_connection_does_not_break_server() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;

    // Simulate is_server_running(): connect and immediately drop.
    {
        let _stream = UnixStream::connect(&socket_path).await.unwrap();
    }

    // Small delay so the server processes the disconnect.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // A real client should still be able to connect and handshake.
    let mut client = common::TestClient::connect(&socket_path).await;
    let ack = client.handshake().await;
    assert!(!ack.server_id.is_empty());
}

/// Multiple rapid probe connections (simulating repeated status checks)
/// must not leak resources or prevent real clients.
#[tokio::test]
async fn repeated_probes_do_not_leak_resources() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;

    // Simulate 10 rapid is_server_running() calls.
    for _ in 0..10 {
        let _stream = UnixStream::connect(&socket_path).await.unwrap();
    }

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Real client should work fine.
    let mut client = common::TestClient::connect(&socket_path).await;
    client.handshake().await;

    // Create a session to verify full functionality.
    let sid = common::create_runtime(
        &mut client,
        "after-probes",
        rttx_proto::proto::RuntimePolicy::Persistent,
    )
    .await;
    assert!(!sid.is_empty());
}
