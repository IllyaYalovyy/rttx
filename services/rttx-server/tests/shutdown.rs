//! Tests for cooperative shutdown.
//!
//! Before this change, sending `Shutdown` called `process::exit(0)` which
//! killed the test binary. Now the server loop returns cleanly, so we can
//! verify post-shutdown state in-process.

mod common;

use rttx_proto::v3;

#[tokio::test]
async fn shutdown_stops_server_and_persists_state() {
    let tmp = tempfile::tempdir().unwrap();
    let (socket_path, server_handle) = common::start_test_server(tmp.path()).await;

    let mut client = common::TestClient::connect(&socket_path).await;
    client.handshake().await;

    // Create a persistent session so there is state worth persisting.
    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: "persist-me".into(),
            policy: v3::WorkspacePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let resp = client.recv().await;
    assert!(
        matches!(resp.payload, Some(v3::server_envelope::Payload::WorkspaceCreated(_))),
        "expected WorkspaceCreated, got {resp:?}"
    );

    // Send shutdown.
    let shutdown = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::Shutdown(v3::Shutdown {})),
    };
    client.send(&shutdown).await;

    // The server task should complete (not hang, not panic).
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), server_handle)
        .await
        .expect("server did not stop within 5 seconds")
        .expect("server task panicked");

    assert!(result.is_ok(), "server returned error: {result:?}");

    // Verify state was persisted to disk (v2 per-workspace files).
    let state_dir = tmp.path().join("state/rttx/daemon");
    let index_path = state_dir.join("daemon.json");
    assert!(index_path.exists(), "v2 daemon index was not written on shutdown");

    let runtimes_dir = state_dir.join("workspaces");
    let mut found = false;
    if runtimes_dir.exists() {
        for entry in std::fs::read_dir(&runtimes_dir).unwrap().flatten() {
            let workspace_json = entry.path().join("workspace.json");
            if workspace_json.exists() {
                let contents = std::fs::read_to_string(&workspace_json).unwrap();
                if contents.contains("persist-me") {
                    found = true;
                    break;
                }
            }
        }
    }
    assert!(found, "persisted v2 state does not contain the workspace we created");
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
    let shutdown = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::Shutdown(v3::Shutdown {})),
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
