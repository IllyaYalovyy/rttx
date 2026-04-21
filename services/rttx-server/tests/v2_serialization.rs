//! Integration tests for v2 per-runtime serialization (RFC-022 Step 3).
//!
//! Verifies that the daemon writes per-runtime files with symlink backup,
//! loads from v2 on restart, and falls back to v1 when no v2 state exists.

mod common;

use common::{TestClient, start_test_server, wait_for_state_containing};
use rttx_proto::proto;
use rttx_server::state::{layout, persistence, types::RUNTIME_FILE_SCHEMA_VERSION};
use std::time::Duration;

/// After creating a persistent runtime, the daemon writes both v1 state.json
/// and v2 per-runtime files.
#[tokio::test]
async fn serialization_writes_v2_runtime_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "v2-write-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    })
    .await;
    let _runtime_id = match c.recv().await.msg {
        Some(proto::server_message::Msg::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    // Wait for serialization tick to write state.
    wait_for_state_containing(&tmp.path().join("cache"), "v2-write-test", Duration::from_secs(10))
        .await;

    // Verify v2 daemon index exists.
    let state_dir = tmp.path().join("state/rttx/daemon");
    let index_path = layout::daemon_index(&state_dir);
    assert!(index_path.exists(), "daemon.json should exist at {}", index_path.display());

    // Verify daemon index content.
    let result = persistence::load_all(&state_dir).expect("v2 state should be loadable");
    assert_eq!(result.runtimes.len(), 1);
    assert_eq!(result.runtimes[0].spec.name, "v2-write-test");
    assert_eq!(result.runtimes[0].schema_version, RUNTIME_FILE_SCHEMA_VERSION);
    assert!(result.failed_ids.is_empty());
}

/// After two daemon index writes (triggered by runtime ID changes), the
/// .bak symlink and .prev file exist.
#[tokio::test]
async fn serialization_creates_backup_symlink() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    // First runtime — triggers first daemon index write.
    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "bak-test".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    })
    .await;
    let _runtime_id = match c.recv().await.msg {
        Some(proto::server_message::Msg::RuntimeCreated(sc)) => sc.runtime_id,
        other => panic!("expected RuntimeCreated, got {other:?}"),
    };

    // Wait for first serialization tick.
    wait_for_state_containing(&tmp.path().join("cache"), "bak-test", Duration::from_secs(10)).await;

    // Second runtime — changes runtime IDs, triggers second daemon index write.
    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
            name: "bak-test-2".into(),
            policy: proto::RuntimePolicy::Persistent as i32,
        })),
    })
    .await;
    let _ = c.recv().await; // RuntimeCreated

    // Wait for second serialization tick to write the updated index.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let state_dir = tmp.path().join("state/rttx/daemon");
    let index_path = layout::daemon_index(&state_dir);
    let bak_path = index_path.with_extension("bak");
    let prev_path = index_path.with_extension("prev");

    assert!(bak_path.is_symlink(), ".bak symlink should exist");
    assert!(prev_path.exists(), ".prev file should exist");
}

/// Restart loads from v2 state (not v1) when both exist.
#[tokio::test]
async fn restart_prefers_v2_over_v1() {
    let tmp = tempfile::TempDir::new().unwrap();

    // Phase 1: create runtime, let serialization write both v1 and v2.
    let runtime_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
                name: "v2-preferred".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
        runtime_id = match c.recv().await.msg {
            Some(proto::server_message::Msg::RuntimeCreated(sc)) => sc.runtime_id,
            other => panic!("expected RuntimeCreated, got {other:?}"),
        };

        wait_for_state_containing(
            &tmp.path().join("cache"),
            "v2-preferred",
            Duration::from_secs(10),
        )
        .await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Corrupt v1 state.json to prove v2 is used.
    let v1_path = tmp.path().join("cache/state.json");
    std::fs::write(&v1_path, "corrupted v1").unwrap();

    // Phase 2: restart — should load from v2 successfully.
    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ListRuntimes(proto::ListRuntimes {})),
        })
        .await;
        let runtimes = match c.recv().await.msg {
            Some(proto::server_message::Msg::RuntimeList(sl)) => sl.runtimes,
            other => panic!("expected RuntimeList, got {other:?}"),
        };
        assert_eq!(runtimes.len(), 1);
        assert_eq!(runtimes[0].id, runtime_id);
        assert_eq!(runtimes[0].name, "v2-preferred");
    }
}

/// When no v2 state exists but v1 does, the daemon falls back to v1.
#[tokio::test]
async fn fallback_to_v1_when_no_v2_state() {
    let tmp = tempfile::TempDir::new().unwrap();

    // Phase 1: create runtime, let serialization write.
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
                name: "v1-fallback".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
        let _ = c.recv().await; // RuntimeCreated

        wait_for_state_containing(
            &tmp.path().join("cache"),
            "v1-fallback",
            Duration::from_secs(10),
        )
        .await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Remove v2 state directory entirely.
    let state_dir = tmp.path().join("state");
    if state_dir.exists() {
        std::fs::remove_dir_all(&state_dir).unwrap();
    }

    // Phase 2: restart — should fall back to v1.
    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ListRuntimes(proto::ListRuntimes {})),
        })
        .await;
        let runtimes = match c.recv().await.msg {
            Some(proto::server_message::Msg::RuntimeList(sl)) => sl.runtimes,
            other => panic!("expected RuntimeList, got {other:?}"),
        };
        assert_eq!(runtimes.len(), 1);
        assert_eq!(runtimes[0].name, "v1-fallback");
    }
}

/// When neither v1 nor v2 state exists, the daemon starts fresh.
#[tokio::test]
async fn fresh_start_when_no_state() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::ListRuntimes(proto::ListRuntimes {})),
    })
    .await;
    let runtimes = match c.recv().await.msg {
        Some(proto::server_message::Msg::RuntimeList(sl)) => sl.runtimes,
        other => panic!("expected RuntimeList, got {other:?}"),
    };
    assert!(runtimes.is_empty(), "fresh start should have no runtimes");
}

/// Corrupt v2 runtime file is skipped; other runtimes still load.
#[tokio::test]
async fn corrupt_v2_runtime_skipped_not_fatal() {
    let tmp = tempfile::TempDir::new().unwrap();

    // Phase 1: create two runtimes.
    let rt1_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
                name: "good-runtime".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
        rt1_id = match c.recv().await.msg {
            Some(proto::server_message::Msg::RuntimeCreated(sc)) => sc.runtime_id,
            other => panic!("expected RuntimeCreated, got {other:?}"),
        };

        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::CreateRuntime(proto::CreateRuntime {
                name: "bad-runtime".into(),
                policy: proto::RuntimePolicy::Persistent as i32,
            })),
        })
        .await;
        let _ = c.recv().await; // RuntimeCreated

        wait_for_state_containing(
            &tmp.path().join("cache"),
            "bad-runtime",
            Duration::from_secs(10),
        )
        .await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Corrupt the second runtime's file.
    let state_dir = tmp.path().join("state/rttx/daemon");
    let result = persistence::load_all(&state_dir).unwrap();
    let bad_rt = result.runtimes.iter().find(|r| r.spec.name == "bad-runtime").unwrap();
    let bad_path = layout::runtime_file(&state_dir, bad_rt.spec.id);
    std::fs::write(&bad_path, "not valid json").unwrap();
    // Also remove .prev so backup can't save it
    let _ = std::fs::remove_file(bad_path.with_extension("prev"));

    // Phase 2: restart — good runtime should survive.
    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::ListRuntimes(proto::ListRuntimes {})),
        })
        .await;
        let runtimes = match c.recv().await.msg {
            Some(proto::server_message::Msg::RuntimeList(sl)) => sl.runtimes,
            other => panic!("expected RuntimeList, got {other:?}"),
        };
        assert_eq!(runtimes.len(), 1, "only the good runtime should survive");
        assert_eq!(runtimes[0].id, rt1_id);
        assert_eq!(runtimes[0].name, "good-runtime");
    }
}
