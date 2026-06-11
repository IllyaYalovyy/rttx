//! Integration tests for workspace directory cleanup (RFC-022 §7, RFC-031 §8).

mod common;

use common::{TestClient, start_test_server, wait_for_state_file};
use rttx_proto::{bytes_to_uuid, uuid_to_bytes, v3};
use std::time::Duration;

/// After terminating a workspace, its v2 state directory should be removed.
#[tokio::test]
async fn terminate_workspace_cleans_up_v2_directory() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (socket_path, _handle) = start_test_server(tmp.path()).await;
    let state_dir = tmp.path().join("state/rttx/daemon");

    let mut client = TestClient::connect(&socket_path).await;
    client.handshake().await;

    // Create a persistent workspace.
    let create = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: "cleanup-test".into(),
            policy: v3::WorkspacePolicy::Persistent as i32,
        })),
    };
    client.send(&create).await;
    let resp = client.recv_or_timeout().await;
    let runtime_id = match resp.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(rc)) => {
            bytes_to_uuid(&rc.runtime_id).unwrap()
        }
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    // Wait for serialization to write the workspace directory.
    wait_for_state_file(tmp.path(), Duration::from_secs(5)).await;
    // Give the v2 serialization loop time to write.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let runtime_dir = state_dir.join("workspaces").join(runtime_id.to_string());
    assert!(runtime_dir.exists(), "workspace directory should exist after creation");

    // Attach so we can terminate.
    let attach = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::AttachWorkspace(v3::AttachWorkspace {
            runtime_id: uuid_to_bytes(runtime_id),
            attach_mode: v3::WorkspaceAttachMode::ReadWrite as i32,
        })),
    };
    client.send(&attach).await;
    let _snap = client.recv_or_timeout().await;

    // Terminate the workspace.
    let terminate = v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::TerminateWorkspace(v3::TerminateWorkspace {
            runtime_id: uuid_to_bytes(runtime_id),
        })),
    };
    client.send(&terminate).await;
    let resp = client.recv_or_timeout().await;
    assert!(
        matches!(resp.payload, Some(v3::server_envelope::Payload::WorkspaceTerminated(_))),
        "expected WorkspaceTerminated"
    );

    // Wait for background cleanup thread.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!runtime_dir.exists(), "workspace directory should be removed after termination");
}

/// On startup the daemon no longer sweeps unreferenced workspace directories
/// (RFC-031 §8). The startup orphan quarantine is deleted: a directory not in
/// the daemon index is simply ignored, never moved into `.orphans/`.
#[tokio::test]
async fn startup_does_not_quarantine_unreferenced_directories() {
    let tmp = tempfile::TempDir::new().unwrap();
    let state_dir = tmp.path().join("state/rttx/daemon");
    std::fs::create_dir_all(&state_dir).unwrap();

    let known_id = uuid::Uuid::new_v4();
    let orphan_id = uuid::Uuid::new_v4();

    // Create a valid v2 workspace file for the known workspace.
    let runtimes_dir = state_dir.join("workspaces");
    let known_dir = runtimes_dir.join(known_id.to_string());
    std::fs::create_dir_all(&known_dir).unwrap();
    let known_rf = serde_json::json!({
        "schema_version": 2,
        "spec": {
            "id": known_id.to_string(),
            "name": "known",
            "policy": "persistent",
            "created_at": { "secs_since_epoch": 1_700_000_000, "nanos_since_epoch": 0 },
            "tree": { "root": null, "default_active": null },
            "panes": []
        },
        "instance": {
            "revision": 1,
            "last_active_at": { "secs_since_epoch": 1_700_000_000, "nanos_since_epoch": 0 },
            "last_snapshot_at": { "secs_since_epoch": 1_700_000_000, "nanos_since_epoch": 0 }
        }
    });
    std::fs::write(
        known_dir.join("workspace.json"),
        serde_json::to_string_pretty(&known_rf).unwrap(),
    )
    .unwrap();

    // Create a directory not referenced by the daemon index.
    let orphan_dir = runtimes_dir.join(orphan_id.to_string());
    std::fs::create_dir_all(&orphan_dir).unwrap();
    std::fs::write(orphan_dir.join("workspace.json"), "{}").unwrap();

    // Write daemon index referencing only the known workspace.
    let index = serde_json::json!({
        "schema_version": 1,
        "server_version": "0.4.2",
        "runtime_ids": [known_id.to_string()],
        "created_at": { "secs_since_epoch": 1_700_000_000, "nanos_since_epoch": 0 },
        "last_serialized_at": { "secs_since_epoch": 1_700_000_000, "nanos_since_epoch": 0 }
    });
    std::fs::write(state_dir.join("daemon.json"), serde_json::to_string_pretty(&index).unwrap())
        .unwrap();

    // Start the server — no sweep runs during load.
    let (_socket_path, _handle) = start_test_server(tmp.path()).await;

    // The unreferenced directory is left untouched: not moved, not quarantined.
    assert!(orphan_dir.exists(), "unreferenced directory must be left in place");
    assert!(
        !runtimes_dir.join(".orphans").exists(),
        "startup must never create the deleted .orphans/ quarantine"
    );

    // Known workspace directory should still exist.
    assert!(known_dir.exists(), "known workspace directory should remain");
}
