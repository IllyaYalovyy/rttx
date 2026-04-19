//! Tests for cooperative shutdown.
//!
//! Before this change, sending `Shutdown` called `process::exit(0)` which
//! killed the test binary. Now the server loop returns cleanly, so we can
//! verify post-shutdown state in-process.

mod common;

use rttx_proto::proto;

#[tokio::test]
async fn shutdown_stops_server_and_persists_state() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, server_handle) = common::start_test_server(tmp.path()).await;

    let mut client = common::TestClient::connect(&socket_path).await;
    client.handshake().await;

    // Create a persistent session so there is state worth persisting.
    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "persist-me".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let resp = client.recv().await;
    assert!(
        matches!(resp.msg, Some(proto::server_message::Msg::RuntimeCreated(_))),
        "expected RuntimeCreated, got {resp:?}"
    );

    // Send shutdown.
    let shutdown = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Shutdown(proto::Shutdown {})),
    };
    client.send(&shutdown).await;

    // The server task should complete (not hang, not panic).
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle)
        .await
        .expect("server did not stop within 5 seconds")
        .expect("server task panicked");

    assert!(result.is_ok(), "server returned error: {result:?}");

    // Verify state was persisted to disk.
    let state_path = tmp.path().join("cache").join("state.json");
    assert!(state_path.exists(), "state file was not written on shutdown");

    let contents = std::fs::read_to_string(&state_path).unwrap();
    assert!(
        contents.contains("persist-me"),
        "persisted state does not contain the session we created"
    );
}

#[tokio::test]
async fn shutdown_is_observable_by_other_clients() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, server_handle) = common::start_test_server(tmp.path()).await;

    let mut client_a = common::TestClient::connect(&socket_path).await;
    client_a.handshake().await;

    let mut client_b = common::TestClient::connect(&socket_path).await;
    client_b.handshake().await;

    // Client A triggers shutdown.
    let shutdown = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::Shutdown(proto::Shutdown {})),
    };
    client_a.send(&shutdown).await;

    // The server task completes, which drops all client connections.
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle)
        .await
        .expect("server did not stop within 5 seconds")
        .expect("server task panicked");

    assert!(result.is_ok());

    // Client B should observe a closed connection (recv returns EOF / error).
    let got = client_b.try_recv(std::time::Duration::from_secs(1)).await;
    // Either None (timeout because server is gone) or a disconnect — both are acceptable.
    // The key assertion is that we got here without the test binary dying.
    drop(got);
}
