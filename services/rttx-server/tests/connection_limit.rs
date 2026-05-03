//! Integration test: connection limit semaphore prevents unbounded
//! client spawning (#826).

mod common;

use common::{TestClient, start_test_server};
use rttx_proto::proto;
use rttx_server::server::MAX_CONCURRENT_CLIENTS;

/// Normal client usage (well under the limit) works without interference.
#[tokio::test]
async fn normal_clients_unaffected_by_connection_limit() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    // Connect several clients — all should succeed.
    let mut clients = Vec::new();
    for _ in 0..5 {
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;
        clients.push(c);
    }

    // Each client can create a runtime.
    for (i, client) in clients.iter_mut().enumerate() {
        let name = format!("ws-{i}");
        common::create_runtime(client, &name, proto::RuntimePolicy::Ephemeral).await;
    }

    // Verify all runtimes are visible.
    let runtimes = common::list_runtimes(&mut clients[0]).await;
    assert_eq!(runtimes.len(), 5, "all 5 runtimes should be listed");
}

/// `MAX_CONCURRENT_CLIENTS` is exported and has a sane value.
#[test]
fn connection_limit_constant_is_exported_and_sane() {
    const { assert!(MAX_CONCURRENT_CLIENTS >= 64) };
    const { assert!(MAX_CONCURRENT_CLIENTS <= 1024) };
}
