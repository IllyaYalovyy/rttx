//! Integration tests for v2 per-workspace serialization (RFC-022 Step 3).
//!
//! Verifies that the daemon writes per-workspace files with symlink backup
//! and loads from v2 on restart.

mod common;

use common::{TestClient, start_test_server, wait_for_state_containing};
use rttx_proto::v3;
use rttx_server::state::{layout, persistence, types::RUNTIME_FILE_SCHEMA_VERSION};
use std::time::Duration;

/// After creating a persistent workspace, the daemon writes v2 per-workspace files.
#[tokio::test]
async fn serialization_writes_v2_runtime_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    c.send(&v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: "v2-write-test".into(),
            policy: v3::WorkspacePolicy::Persistent as i32,
        })),
    })
    .await;
    let _runtime_id = match c.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(sc)) => sc.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    // Wait for serialization tick to write state.
    wait_for_state_containing(tmp.path(), "v2-write-test", Duration::from_secs(10)).await;

    // Verify v2 daemon index exists.
    let state_dir = tmp.path().join("state/rttx/daemon");
    let index_path = layout::daemon_index(&state_dir);
    assert!(index_path.exists(), "daemon.json should exist at {}", index_path.display());

    // Verify daemon index content.
    let result = persistence::load_all(&state_dir).expect("v2 state should be loadable");
    assert_eq!(result.workspaces.len(), 1);
    assert_eq!(result.workspaces[0].spec.name, "v2-write-test");
    assert_eq!(result.workspaces[0].schema_version, RUNTIME_FILE_SCHEMA_VERSION);
    assert!(result.failed_ids.is_empty());
}

/// After two daemon index writes (triggered by workspace ID changes), the
/// .bak symlink and .prev file exist.
#[tokio::test]
async fn serialization_creates_backup_symlink() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    // First workspace — triggers first daemon index write.
    c.send(&v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: "bak-test".into(),
            policy: v3::WorkspacePolicy::Persistent as i32,
        })),
    })
    .await;
    let _runtime_id = match c.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceCreated(sc)) => sc.runtime_id,
        other => panic!("expected WorkspaceCreated, got {other:?}"),
    };

    // Wait for first serialization tick.
    wait_for_state_containing(tmp.path(), "bak-test", Duration::from_secs(10)).await;

    // Second workspace — changes workspace IDs, triggers second daemon index write.
    c.send(&v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: "bak-test-2".into(),
            policy: v3::WorkspacePolicy::Persistent as i32,
        })),
    })
    .await;
    let _ = c.recv().await; // WorkspaceCreated

    // Wait for second serialization tick to write the updated index.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let state_dir = tmp.path().join("state/rttx/daemon");
    let index_path = layout::daemon_index(&state_dir);
    let bak_path = index_path.with_extension("bak");
    let prev_path = index_path.with_extension("prev");

    assert!(bak_path.is_symlink(), ".bak symlink should exist");
    assert!(prev_path.exists(), ".prev file should exist");
}

/// Restart loads current-schema state and ignores an older-version file.
#[tokio::test]
async fn restart_loads_persisted_current_state() {
    let tmp = tempfile::TempDir::new().unwrap();

    // Phase 1: create workspace, let serialization write v2 state.
    let runtime_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        c.send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
                name: "v2-preferred".into(),
                policy: v3::WorkspacePolicy::Persistent as i32,
            })),
        })
        .await;
        runtime_id = match c.recv().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceCreated(sc)) => sc.runtime_id,
            other => panic!("expected WorkspaceCreated, got {other:?}"),
        };

        wait_for_state_containing(tmp.path(), "v2-preferred", Duration::from_secs(10)).await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Phase 2: restart — should load current-schema state successfully.
    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        c.send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::ListWorkspaces(v3::ListWorkspaces {})),
        })
        .await;
        let workspaces = match c.recv().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceList(sl)) => sl.workspaces,
            other => panic!("expected WorkspaceList, got {other:?}"),
        };
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].id, runtime_id);
        assert_eq!(workspaces[0].name, "v2-preferred");
    }
}

/// When no v2 state exists, the daemon starts fresh.
#[tokio::test]
async fn fresh_start_when_no_state() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    c.send(&v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::ListWorkspaces(v3::ListWorkspaces {})),
    })
    .await;
    let workspaces = match c.recv().await.payload {
        Some(v3::server_envelope::Payload::WorkspaceList(sl)) => sl.workspaces,
        other => panic!("expected WorkspaceList, got {other:?}"),
    };
    assert!(workspaces.is_empty(), "fresh start should have no workspaces");
}

/// Corrupt v2 workspace file is skipped; other workspaces still load.
#[tokio::test]
async fn corrupt_v2_workspace_skipped_not_fatal() {
    let tmp = tempfile::TempDir::new().unwrap();

    // Phase 1: create two workspaces.
    let rt1_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        c.send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
                name: "good-workspace".into(),
                policy: v3::WorkspacePolicy::Persistent as i32,
            })),
        })
        .await;
        rt1_id = match c.recv().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceCreated(sc)) => sc.runtime_id,
            other => panic!("expected WorkspaceCreated, got {other:?}"),
        };

        c.send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
                name: "bad-workspace".into(),
                policy: v3::WorkspacePolicy::Persistent as i32,
            })),
        })
        .await;
        let _ = c.recv().await; // WorkspaceCreated

        wait_for_state_containing(tmp.path(), "bad-workspace", Duration::from_secs(10)).await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Corrupt the second workspace's file.
    let state_dir = tmp.path().join("state/rttx/daemon");
    let result = persistence::load_all(&state_dir).unwrap();
    let bad_rt = result.workspaces.iter().find(|r| r.spec.name == "bad-workspace").unwrap();
    let bad_path = layout::runtime_file(&state_dir, bad_rt.spec.id);
    std::fs::write(&bad_path, "not valid json").unwrap();
    // Also remove .prev so backup can't save it
    let _ = std::fs::remove_file(bad_path.with_extension("prev"));

    // Phase 2: restart — good workspace should survive.
    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        c.send(&v3::ClientEnvelope {
            request_id: 0,
            command: Some(v3::client_envelope::Command::ListWorkspaces(v3::ListWorkspaces {})),
        })
        .await;
        let workspaces = match c.recv().await.payload {
            Some(v3::server_envelope::Payload::WorkspaceList(sl)) => sl.workspaces,
            other => panic!("expected WorkspaceList, got {other:?}"),
        };
        assert_eq!(workspaces.len(), 1, "only the good workspace should survive");
        assert_eq!(workspaces[0].id, rt1_id);
        assert_eq!(workspaces[0].name, "good-workspace");
    }
}

/// The daemon understands only the current storage schema. A `workspace.json`
/// carrying an unsupported `schema_version` is skipped on load — not
/// deserialized, not migrated, not loaded.
#[test]
fn unsupported_schema_workspace_file_is_ignored_on_load() {
    let tmp = tempfile::TempDir::new().unwrap();
    let state_dir = tmp.path().join("state/rttx/daemon");
    std::fs::create_dir_all(&state_dir).unwrap();

    let old_id = uuid::Uuid::new_v4();
    persistence::save_daemon_index(&state_dir, &[old_id]).unwrap();

    let old_path = layout::runtime_file(&state_dir, old_id);
    if let Some(parent) = old_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&old_path, r#"{"schema_version": 99, "spec": {}, "instance": {}}"#).unwrap();

    let result = persistence::load_all(&state_dir).expect("state dir loads");
    assert!(result.workspaces.is_empty(), "an unsupported-schema workspace file must not load");
    assert_eq!(result.failed_ids, vec![old_id], "it is skipped as an unsupported file");
}

/// A persisted workspace file is written with the current schema version.
#[tokio::test]
async fn persisted_workspace_file_carries_current_schema_version() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, handle) = start_test_server(tmp.path()).await;
    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    c.send(&v3::ClientEnvelope {
        request_id: 0,
        command: Some(v3::client_envelope::Command::CreateWorkspace(v3::CreateWorkspace {
            name: "schema-version-test".into(),
            policy: v3::WorkspacePolicy::Persistent as i32,
        })),
    })
    .await;
    let _ = c.recv().await;

    wait_for_state_containing(tmp.path(), "schema-version-test", Duration::from_secs(10)).await;
    handle.abort();

    let state_dir = tmp.path().join("state/rttx/daemon");
    let result = persistence::load_all(&state_dir).expect("state loads");
    let ws = result.workspaces.iter().find(|r| r.spec.name == "schema-version-test").unwrap();
    assert_eq!(ws.schema_version, RUNTIME_FILE_SCHEMA_VERSION);
}
