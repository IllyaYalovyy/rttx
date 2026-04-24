//! Integration test verifying the first-run state directory announcement.
//!
//! When no prior state exists, the daemon starts fresh and subsequent
//! state writes land in the expected `$XDG_STATE_HOME/rttx/daemon/`
//! directory (RFC-022 Step 9).

mod common;

use common::{create_runtime, start_test_server};
use rttx_proto::proto;
use rttx_server::state::layout;
use std::time::Duration;

#[tokio::test]
async fn first_run_creates_state_in_state_dir_not_cache() {
    let tmp = tempfile::TempDir::new().unwrap();

    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut c = common::TestClient::connect(&sock).await;
    c.handshake().await;

    let rt_id_bytes =
        create_runtime(&mut c, "first-run-test", proto::RuntimePolicy::Persistent).await;
    let runtime_id = rttx_proto::bytes_to_uuid(&rt_id_bytes).unwrap();

    // Wait for serialization to write state.
    common::wait_for_state_containing(tmp.path(), "first-run-test", Duration::from_secs(10)).await;

    // Verify state landed in the v2 state directory, not the cache.
    let state_dir = tmp.path().join("state/rttx/daemon");
    let rt_path = layout::runtime_file(&state_dir, runtime_id);
    assert!(
        rt_path.exists(),
        "runtime.json should be written to state_dir ({}), not cache_dir",
        rt_path.display()
    );

    let index_path = layout::daemon_index(&state_dir);
    assert!(index_path.exists(), "daemon.json should exist in state_dir");
}

/// Verify that a v1 state.json in the cache directory is not loaded.
/// The v1 fallback was removed; the daemon must start fresh.
#[tokio::test]
async fn v1_state_json_in_cache_is_not_loaded() {
    let tmp = tempfile::TempDir::new().unwrap();

    // Write a v1 state.json with a runtime.
    let cache_dir = tmp.path().join("cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::write(
        cache_dir.join("state.json"),
        r#"{
            "sessions": [{
                "id": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                "name": "v1-ghost",
                "panes": [],
                "active_pane_id": null,
                "command_history": [],
                "policy": "persistent",
                "revision": 1,
                "created_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
                "last_active_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0}
            }],
            "serialized_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
            "server_version": "0.3.0"
        }"#,
    )
    .unwrap();

    let (sock, _handle) = start_test_server(tmp.path()).await;
    let mut c = common::TestClient::connect(&sock).await;
    c.handshake().await;

    let runtimes = common::list_runtimes(&mut c).await;
    assert!(
        runtimes.is_empty(),
        "v1 state.json must not be loaded — daemon should start fresh"
    );
}
