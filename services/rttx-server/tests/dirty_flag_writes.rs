//! Integration tests for dirty-flag writes (RFC-022 §5, issue #720).
//!
//! Verifies that the serialization loop skips clean runtimes and only
//! rewrites the daemon index when the set of runtime IDs changes.

mod common;

use common::{TestClient, start_test_server, wait_for_state_containing};
use rttx_proto::proto;
use rttx_server::state::{layout, persistence};
use std::time::Duration;

/// A clean runtime is not rewritten on subsequent ticks.
/// Verified by checking that the runtime file's mtime does not change
/// after the initial write.
#[tokio::test]
async fn clean_runtime_not_rewritten_on_subsequent_ticks() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "idle-runtime".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    })
    .await;
    let runtime_id_bytes = match c.recv().await.msg {
        Some(proto::server_message::Msg::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };
    let runtime_id = rttx_proto::bytes_to_uuid(&runtime_id_bytes).unwrap();

    // Wait for first serialization tick.
    wait_for_state_containing(tmp.path(), "idle-runtime", Duration::from_secs(10))
        .await;

    let state_dir = tmp.path().join("state/rttx/daemon");
    let rt_path = layout::runtime_file(&state_dir, runtime_id);
    assert!(rt_path.exists(), "runtime file should exist after first tick");

    // Record mtime after first write.
    let mtime_after_first = std::fs::metadata(&rt_path).unwrap().modified().unwrap();

    // Wait for several more ticks — the file should NOT be rewritten.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let mtime_after_idle = std::fs::metadata(&rt_path).unwrap().modified().unwrap();
    assert_eq!(
        mtime_after_first, mtime_after_idle,
        "clean runtime file should not be rewritten on idle ticks"
    );
}

/// A mutation (rename) makes the runtime dirty and triggers a rewrite.
#[tokio::test]
async fn mutation_triggers_rewrite() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "mutable-rt".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    })
    .await;
    let runtime_id_bytes = match c.recv().await.msg {
        Some(proto::server_message::Msg::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };
    let runtime_id = rttx_proto::bytes_to_uuid(&runtime_id_bytes).unwrap();

    // Wait for first write.
    wait_for_state_containing(tmp.path(), "mutable-rt", Duration::from_secs(10))
        .await;

    let state_dir = tmp.path().join("state/rttx/daemon");
    let rt_path = layout::runtime_file(&state_dir, runtime_id);
    let mtime_before_rename = std::fs::metadata(&rt_path).unwrap().modified().unwrap();

    // Wait a moment so mtime granularity doesn't mask the change.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Rename the runtime — this bumps revision and makes it dirty.
    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::RenameRuntime(proto::RenameRuntime {
            runtime_id: runtime_id_bytes,
            name: "renamed-rt".into(),
        })),
    })
    .await;
    let _ = c.recv().await; // RuntimeRenamed

    // Wait for the next serialization tick to pick up the dirty runtime.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let mtime_after_rename = std::fs::metadata(&rt_path).unwrap().modified().unwrap();
    assert!(
        mtime_after_rename > mtime_before_rename,
        "dirty runtime should be rewritten after mutation"
    );

    // Verify the file contains the new name.
    let result = persistence::load_all(&state_dir).unwrap();
    let rt = result.runtimes.iter().find(|r| r.spec.id == runtime_id).unwrap();
    assert_eq!(rt.spec.name, "renamed-rt");
}

/// Daemon index is only rewritten when runtime IDs change.
#[tokio::test]
async fn daemon_index_not_rewritten_when_ids_unchanged() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "index-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    })
    .await;
    let _ = c.recv().await; // RuntimeCreated

    // Wait for first write.
    wait_for_state_containing(tmp.path(), "index-test", Duration::from_secs(10))
        .await;

    let state_dir = tmp.path().join("state/rttx/daemon");
    let index_path = layout::daemon_index(&state_dir);
    let mtime_after_first = std::fs::metadata(&index_path).unwrap().modified().unwrap();

    // Wait for several ticks — index should NOT be rewritten.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let mtime_after_idle = std::fs::metadata(&index_path).unwrap().modified().unwrap();
    assert_eq!(
        mtime_after_first, mtime_after_idle,
        "daemon index should not be rewritten when runtime IDs are unchanged"
    );
}
