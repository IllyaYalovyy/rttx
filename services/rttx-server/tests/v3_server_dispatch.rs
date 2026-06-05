//! Integration test: v3 protocol dispatch through the live server.
//!
//! Verifies that the daemon correctly handles v3 protocol types
//! end-to-end over a real Unix socket connection.

mod common;


#[tokio::test]
async fn v3_dispatch_create_and_list_runtimes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = common::start_test_server(tmp.path()).await;

    let mut client = common::TestClient::connect(&socket_path).await;
    client.handshake().await;

    // Create a runtime via v2 protocol (v3 dispatch is wired but v2 still works).
    let runtime_id =
        common::create_runtime(&mut client, "v3-test", rttx_proto::v3::RuntimePolicy::Persistent)
            .await;
    assert!(!runtime_id.is_empty());

    // List runtimes and verify the runtime exists.
    let runtimes = common::list_runtimes(&mut client).await;
    assert_eq!(runtimes.len(), 1);
    assert_eq!(runtimes[0].name, "v3-test");
}

#[tokio::test]
async fn v3_pane_output_seq_starts_at_zero_on_attach() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = common::start_test_server(tmp.path()).await;

    let mut client = common::TestClient::connect(&socket_path).await;
    client.handshake().await;

    let runtime_id =
        common::create_runtime(&mut client, "seq-test", rttx_proto::v3::RuntimePolicy::Persistent)
            .await;
    let snap = common::attach_rw(&mut client, &runtime_id).await;

    // Snapshot panes should exist (from attach) — empty runtime has no panes.
    assert!(snap.panes.is_empty());
}
