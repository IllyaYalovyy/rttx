//! Integration tests for SSH connection behavior.
//!
//! Verifies that SSH connections fail fast instead of hanging when
//! the remote host is unreachable or auth is not possible.

use rttx::daemon::{DaemonConnection, DaemonError};

#[tokio::test]
async fn ssh_connection_to_unreachable_host_fails_within_timeout() {
    let start = std::time::Instant::now();
    let result = DaemonConnection::connect_ssh("rttx-nonexistent-host-test").await;

    assert!(result.is_err(), "SSH to unreachable host should fail");
    assert!(
        start.elapsed().as_secs() < 15,
        "SSH should fail fast with BatchMode=yes, took {}s",
        start.elapsed().as_secs()
    );

    match result.unwrap_err() {
        DaemonError::Io(_) | DaemonError::Disconnected => {}
        other => panic!("expected Io or Disconnected error, got: {other:?}"),
    }
}
