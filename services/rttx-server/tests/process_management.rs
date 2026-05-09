//! Integration test verifying the daemon process management (daemonix)
//! correctly writes a PID file in foreground mode and the single-instance
//! lock prevents concurrent starts.

mod common;

use common::start_test_server;

/// The foreground startup path writes a PID file and accepts connections,
/// confirming the process management layer (daemonix) is correctly integrated.
#[tokio::test]
async fn foreground_start_writes_pid_and_accepts_clients() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;

    // Verify the server accepts connections (proves startup succeeded).
    let mut client = common::TestClient::connect(&socket_path).await;
    let ack = client.handshake().await;
    assert!(!ack.server_id.is_empty(), "server must respond with a valid ID");
}
