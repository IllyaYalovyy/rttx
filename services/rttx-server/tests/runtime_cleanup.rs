//! Integration tests for runtime directory cleanup and orphan sweep.

mod common;

use common::{TestClient, start_test_server, wait_for_state_file};
use rttx_proto::{bytes_to_uuid, proto, uuid_to_bytes};
use std::time::Duration;

/// After terminating a runtime, its v2 state directory should be removed.
#[tokio::test]
async fn terminate_runtime_cleans_up_v2_directory() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let state_dir = tmp.path().join("state/rttx/daemon");

    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    // Create a persistent runtime.
    let create = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "cleanup-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let resp = client.recv_or_timeout().await;
    let runtime_id = match resp.msg {
        Some(proto::server_message::Msg::RuntimeCreated(rc)) => {
            bytes_to_uuid(&rc.runtime_id).unwrap()
        }
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    // Wait for serialization to write the runtime directory.
    wait_for_state_file(&tmp.path().join("cache"), Duration::from_secs(5)).await;
    // Give the v2 serialization loop time to write.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let runtime_dir = state_dir.join("runtimes").join(runtime_id.to_string());
    assert!(runtime_dir.exists(), "runtime directory should exist after creation");

    // Attach so we can terminate.
    let attach = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    let _snap = client.recv_or_timeout().await;

    // Terminate the runtime.
    let terminate = proto::ClientMessage {
        msg: Some(proto::client_message::Msg::TerminateRuntime(proto::TerminateRuntime {
            runtime_id: uuid_to_bytes(runtime_id),
        })),
    };
    client.send(&terminate).await;
    let resp = client.recv_or_timeout().await;
    assert!(
        matches!(resp.msg, Some(proto::server_message::Msg::RuntimeTerminated(_))),
        "expected RuntimeTerminated"
    );

    // Wait for background cleanup thread.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!runtime_dir.exists(), "runtime directory should be removed after termination");
}

/// On startup, unreferenced runtime directories are moved to .orphans/.
#[tokio::test]
async fn startup_quarantines_orphaned_runtime_directories() {
    let tmp = tempfile::TempDir::new().unwrap();
    let state_dir = tmp.path().join("state/rttx/daemon");
    std::fs::create_dir_all(&state_dir).unwrap();

    let known_id = uuid::Uuid::new_v4();
    let orphan_id = uuid::Uuid::new_v4();

    // Create a valid runtime file for the known runtime.
    let runtimes_dir = state_dir.join("runtimes");
    let known_dir = runtimes_dir.join(known_id.to_string());
    std::fs::create_dir_all(&known_dir).unwrap();
    let known_rf = serde_json::json!({
        "schema_version": 1,
        "spec": {
            "id": known_id.to_string(),
            "name": "known",
            "policy": "persistent",
            "created_at": { "secs_since_epoch": 1_700_000_000, "nanos_since_epoch": 0 },
            "panes": [],
            "active_pane_id": null,
            "command_history": []
        },
        "instance": {
            "revision": 1,
            "last_active_at": { "secs_since_epoch": 1_700_000_000, "nanos_since_epoch": 0 },
            "last_snapshot_at": { "secs_since_epoch": 1_700_000_000, "nanos_since_epoch": 0 }
        }
    });
    std::fs::write(
        known_dir.join("runtime.json"),
        serde_json::to_string_pretty(&known_rf).unwrap(),
    )
    .unwrap();

    // Create an orphan directory (not in daemon index).
    let orphan_dir = runtimes_dir.join(orphan_id.to_string());
    std::fs::create_dir_all(&orphan_dir).unwrap();
    std::fs::write(orphan_dir.join("runtime.json"), "{}").unwrap();

    // Write daemon index referencing only the known runtime.
    let index = serde_json::json!({
        "schema_version": 1,
        "server_version": "0.4.2",
        "runtime_ids": [known_id.to_string()],
        "created_at": { "secs_since_epoch": 1_700_000_000, "nanos_since_epoch": 0 },
        "last_serialized_at": { "secs_since_epoch": 1_700_000_000, "nanos_since_epoch": 0 }
    });
    std::fs::write(state_dir.join("daemon.json"), serde_json::to_string_pretty(&index).unwrap())
        .unwrap();

    // Start the server — it should sweep orphans during load.
    let (_socket_path, _handle) = start_test_server(tmp.path()).await;

    // Orphan should have been moved to .orphans/.
    assert!(!orphan_dir.exists(), "orphan directory should be moved");
    let orphan_dest = runtimes_dir.join(".orphans").join(orphan_id.to_string());
    assert!(orphan_dest.exists(), "orphan should be in .orphans/");

    // Known runtime directory should still exist.
    assert!(known_dir.exists(), "known runtime directory should remain");
}
