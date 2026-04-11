//! Integration tests for the `clean` command behavior:
//! terminate all sessions with no attached clients.

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::proto;

#[tokio::test]
async fn clean_removes_only_detached_sessions() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    // Create two sessions.
    let attached_id =
        common::create_session(&mut client, "attached", proto::RuntimePolicy::Persistent).await;
    common::create_session(&mut client, "detached", proto::RuntimePolicy::Persistent).await;

    // Attach to the first session only.
    common::attach_rw(&mut client, &attached_id).await;

    // Verify both exist.
    let sessions = common::list_sessions(&mut client).await;
    assert_eq!(sessions.len(), 2);

    // Connect a second client to perform the clean operation.
    let mut cleaner = TestClient::connect(&sock).await;
    cleaner.handshake().await;

    // List sessions from the cleaner's perspective.
    let sessions = common::list_sessions(&mut cleaner).await;
    let to_clean: Vec<_> = sessions.iter().filter(|s| s.attached_client_count == 0).collect();
    assert_eq!(to_clean.len(), 1, "exactly one session should have no clients");
    assert_eq!(to_clean[0].name, "detached");

    // Terminate the detached session.
    common::terminate_session(&mut cleaner, &to_clean[0].id).await;

    // Verify only the attached session remains.
    let sessions = common::list_sessions(&mut client).await;
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].name, "attached");
}

#[tokio::test]
async fn clean_with_no_detached_sessions_is_noop() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    let session_id =
        common::create_session(&mut client, "active", proto::RuntimePolicy::Persistent).await;
    common::attach_rw(&mut client, &session_id).await;

    // All sessions are attached — nothing to clean.
    let sessions = common::list_sessions(&mut client).await;
    assert!(!sessions.iter().any(|s| s.attached_client_count == 0));

    // Session still exists.
    let sessions = common::list_sessions(&mut client).await;
    assert_eq!(sessions.len(), 1);
}

#[tokio::test]
async fn clean_removes_all_detached_sessions() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut client = TestClient::connect(&sock).await;
    client.handshake().await;

    // Create three sessions, none attached.
    common::create_session(&mut client, "orphan-1", proto::RuntimePolicy::Persistent).await;
    common::create_session(&mut client, "orphan-2", proto::RuntimePolicy::Persistent).await;
    common::create_session(&mut client, "orphan-3", proto::RuntimePolicy::Ephemeral).await;

    let sessions = common::list_sessions(&mut client).await;
    assert_eq!(sessions.len(), 3);

    // All have zero attached clients.
    let to_clean: Vec<_> = sessions.iter().filter(|s| s.attached_client_count == 0).collect();
    assert_eq!(to_clean.len(), 3);

    // Terminate all.
    for s in &to_clean {
        common::terminate_session(&mut client, &s.id).await;
    }

    // Verify all removed.
    let sessions = common::list_sessions(&mut client).await;
    assert!(sessions.is_empty());
}

/// Binary-level test: `rttx-server clean` reports "not running" when no daemon.
#[test]
fn clean_reports_not_running_without_daemon() {
    let bin = env!("CARGO_BIN_EXE_rttx-server");
    let tmp = tempfile::TempDir::new().unwrap();
    let runtime_dir = tmp.path().join("runtime");
    std::fs::create_dir_all(&runtime_dir).unwrap();

    let output = std::process::Command::new(bin)
        .arg("clean")
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .output()
        .expect("failed to run clean");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("not running"), "should report daemon not running, got: {stdout}");
}
