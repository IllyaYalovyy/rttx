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
    common::wait_for_state_containing(
        tmp.path(),
        "first-run-test",
        Duration::from_secs(10),
    )
    .await;

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
