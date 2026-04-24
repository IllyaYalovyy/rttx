//! Integration tests for `ScreenSnapshotV1` serialization (RFC-022 Step 5).
//!
//! Verifies that the daemon writes screen snapshots alongside per-runtime
//! files and that corrupt snapshots do not block runtime loading.

mod common;

use common::{TestClient, start_test_server, wait_for_state_containing};
use rttx_proto::{bytes_to_uuid, proto};
use rttx_server::state::{layout, persistence};
use std::time::Duration;

/// After creating a persistent runtime with a pane, the daemon writes
/// screen snapshot files at `screen/<pane_id>.snap`.
#[tokio::test]
async fn serialization_writes_screen_snapshots() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    // Create a persistent runtime.
    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "snap-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    })
    .await;
    let runtime_id_bytes = match c.recv().await.msg {
        Some(proto::server_message::Msg::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };
    let runtime_id = bytes_to_uuid(&runtime_id_bytes).unwrap();

    // Attach to the runtime (ReadWrite).
    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::AttachRuntime(proto::AttachRuntime {
            runtime_id: runtime_id_bytes.clone(),
            attach_mode: proto::RuntimeAttachMode::ReadWrite as i32,
        })),
    })
    .await;
    // Consume the Snapshot response (empty panes at this point).
    let _ = c.recv().await;

    // Create a pane.
    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
            runtime_id: runtime_id_bytes.clone(),
            cols: 80,
            rows: 24,
            no_persist: None,
            ..Default::default()
        })),
    })
    .await;
    let pane_id = match c.recv().await.msg {
        Some(proto::server_message::Msg::PaneCreated(pc)) => bytes_to_uuid(&pc.pane_id).unwrap(),
        other => panic!("expected PaneCreated, got {other:?}"),
    };

    // Wait for serialization tick to write state.
    wait_for_state_containing(tmp.path(), "snap-test", Duration::from_secs(10))
        .await;

    // Verify screen snapshot file exists.
    let state_dir = tmp.path().join("state/rttx/daemon");
    let snap_path = layout::screen_snapshot(&state_dir, runtime_id, pane_id);
    assert!(snap_path.exists(), "screen snapshot should exist at {}", snap_path.display());

    // Verify snapshot content is valid.
    let snap = persistence::load_screen_snapshot(&state_dir, runtime_id, pane_id)
        .expect("screen snapshot should be loadable");
    assert_eq!(snap.pane_id, pane_id);
    assert_eq!(snap.cols, 80);
    assert_eq!(snap.rows, 24);
    assert_eq!(snap.schema_version, rttx_server::state::types::SCREEN_SNAPSHOT_SCHEMA_VERSION);
}

/// A corrupt screen snapshot does not prevent runtime loading.
#[tokio::test]
async fn corrupt_screen_snapshot_does_not_block_runtime_load() {
    let tmp = tempfile::TempDir::new().unwrap();
    let state_dir = tmp.path().join("state/rttx/daemon");

    let runtime_id = uuid::Uuid::new_v4();
    let pane_id = uuid::Uuid::new_v4();

    let rf = rttx_server::state::types::RuntimeFileV1 {
        schema_version: rttx_server::state::types::RUNTIME_FILE_SCHEMA_VERSION,
        spec: rttx_server::state::types::RuntimeSpecV1 {
            id: runtime_id,
            name: "corrupt-snap-test".into(),
            policy: rttx_server::runtime::RuntimePolicy::Persistent,
            created_at: std::time::SystemTime::now(),
            panes: vec![rttx_server::state::types::PaneSpecV1 {
                id: pane_id,
                cwd: Some("/tmp".into()),
                title: None,
                exit_status: None,
                cols: 80,
                rows: 24,
                no_persist: false,
            }],
            active_pane_id: None,
            command_history: vec![],
        },
        instance: rttx_server::state::types::RuntimeInstanceV1 {
            revision: 1,
            last_active_at: std::time::SystemTime::now(),
            last_snapshot_at: std::time::SystemTime::now(),
        },
    };

    persistence::save_daemon_index(&state_dir, &[runtime_id]).unwrap();
    persistence::save_runtime(&state_dir, &rf).unwrap();

    // Write a corrupt screen snapshot.
    let snap_path = layout::screen_snapshot(&state_dir, runtime_id, pane_id);
    std::fs::create_dir_all(snap_path.parent().unwrap()).unwrap();
    std::fs::write(&snap_path, "not valid json").unwrap();

    // Loading the snapshot should return None (not crash).
    let snap = persistence::load_screen_snapshot(&state_dir, runtime_id, pane_id);
    assert!(snap.is_none(), "corrupt snapshot should return None");

    // The runtime itself should still load fine.
    let result = persistence::load_all(&state_dir).expect("v2 state should load");
    assert_eq!(result.runtimes.len(), 1);
    assert_eq!(result.runtimes[0].spec.id, runtime_id);
}
