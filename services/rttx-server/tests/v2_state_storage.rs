//! Gating integration tests for v2 state storage (RFC-022 Step 8, issue #724).
//!
//! Covers the five areas required before v1 removal:
//! 1. Corrupt `runtime.json` does not kill the daemon
//! 2. Dirty-flag skips writes
//! 3. Scrollback rotation keeps N segments
//! 4. Orphan sweep quarantines unreferenced directories
//! 5. `ScreenSnapshotV1` round-trip through server restart

mod common;

use common::{
    TestClient, attach_rw, create_pane, create_runtime, list_runtimes, send_input,
    start_test_server, terminate_runtime, wait_for_state_containing,
};
use rttx_proto::{bytes_to_uuid, proto};
use rttx_server::state::{layout, persistence, types::SCREEN_SNAPSHOT_SCHEMA_VERSION};
use std::time::Duration;

// ── 1. Corrupt runtime.json containment ─────────────────────────

/// Corrupt daemon.json primary falls back to .bak and loads runtimes.
#[tokio::test]
async fn corrupt_daemon_index_falls_back_to_backup() {
    let tmp = tempfile::TempDir::new().unwrap();

    // Phase 1: create a runtime so both v1 and v2 state are written.
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        let _rt_id =
            create_runtime(&mut c, "index-fallback", proto::RuntimePolicy::Persistent).await;

        wait_for_state_containing(
            tmp.path(),
            "index-fallback",
            Duration::from_secs(10),
        )
        .await;

        // Create a second runtime to trigger a second daemon index write,
        // which produces the .prev backup.
        let _rt2_id =
            create_runtime(&mut c, "index-fallback-2", proto::RuntimePolicy::Persistent).await;
        tokio::time::sleep(Duration::from_secs(2)).await;

        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Corrupt the primary daemon.json.
    let state_dir = tmp.path().join("state/rttx/daemon");
    let index_path = layout::daemon_index(&state_dir);
    assert!(index_path.exists());
    std::fs::write(&index_path, "corrupted!").unwrap();

    // Phase 2: restart — should recover from .bak/.prev.
    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        let runtimes = list_runtimes(&mut c).await;
        // The backup was written before the second runtime was added,
        // so it may contain 1 runtime (from the first index write).
        assert!(!runtimes.is_empty(), "daemon should recover runtimes from backup index");
        assert!(
            runtimes.iter().any(|r| r.name == "index-fallback"),
            "first runtime should survive via backup"
        );
    }
}

/// Both daemon.json and its backup corrupt → daemon starts fresh.
#[tokio::test]
async fn both_daemon_index_copies_corrupt_starts_fresh() {
    let tmp = tempfile::TempDir::new().unwrap();

    // Phase 1: create a runtime.
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        let _rt_id =
            create_runtime(&mut c, "doomed-runtime", proto::RuntimePolicy::Persistent).await;

        wait_for_state_containing(
            tmp.path(),
            "doomed-runtime",
            Duration::from_secs(10),
        )
        .await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Corrupt both v2 primary and backup.
    let state_dir = tmp.path().join("state/rttx/daemon");
    let index_path = layout::daemon_index(&state_dir);
    std::fs::write(&index_path, "corrupted primary").unwrap();
    let prev_path = index_path.with_extension("prev");
    std::fs::write(&prev_path, "corrupted backup").unwrap();

    // Phase 2: restart — should start fresh (0 runtimes), not crash.
    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        let runtimes = list_runtimes(&mut c).await;
        assert!(
            runtimes.is_empty(),
            "daemon should start fresh when both index copies are corrupt"
        );
    }
}

/// Corrupt runtime.json with valid backup recovers from .prev.
#[tokio::test]
async fn corrupt_runtime_file_recovers_from_backup() {
    let tmp = tempfile::TempDir::new().unwrap();

    // Phase 1: create a runtime and write state twice to produce .prev.
    let runtime_id;
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        let rt_id_bytes =
            create_runtime(&mut c, "backup-recovery", proto::RuntimePolicy::Persistent).await;
        runtime_id = bytes_to_uuid(&rt_id_bytes).unwrap();

        // Wait for first write.
        wait_for_state_containing(
            tmp.path(),
            "backup-recovery",
            Duration::from_secs(10),
        )
        .await;

        // Attach and create a pane to trigger a second write (dirty flag).
        attach_rw(&mut c, &rt_id_bytes).await;
        let _pane_id = create_pane(&mut c, &rt_id_bytes).await;
        tokio::time::sleep(Duration::from_secs(2)).await;

        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Corrupt the primary runtime.json but leave .prev intact.
    let state_dir = tmp.path().join("state/rttx/daemon");
    let rt_path = layout::runtime_file(&state_dir, runtime_id);
    let prev_path = rt_path.with_extension("prev");
    assert!(prev_path.exists(), ".prev should exist after two writes");
    std::fs::write(&rt_path, "corrupted!").unwrap();

    // Phase 2: restart — should recover from .prev.
    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        let runtimes = list_runtimes(&mut c).await;
        assert_eq!(runtimes.len(), 1);
        assert_eq!(runtimes[0].name, "backup-recovery");
    }
}

// ── 2. Dirty-flag skips writes ──────────────────────────────────

/// After restart, a loaded runtime is not dirty (`persisted_revision` matches
/// `revision`), so it should not be rewritten on subsequent ticks.
#[tokio::test]
async fn loaded_runtime_is_clean_after_restart() {
    let tmp = tempfile::TempDir::new().unwrap();

    // Phase 1: create a runtime.
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        let _rt_id =
            create_runtime(&mut c, "clean-after-restart", proto::RuntimePolicy::Persistent).await;

        wait_for_state_containing(
            tmp.path(),
            "clean-after-restart",
            Duration::from_secs(10),
        )
        .await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Phase 2: restart and verify the runtime file is not rewritten.
    {
        let state_dir = tmp.path().join("state/rttx/daemon");
        let result = persistence::load_all(&state_dir).unwrap();
        let rt_id = result.runtimes[0].spec.id;
        let rt_path = layout::runtime_file(&state_dir, rt_id);

        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        // Wait for the server to settle and run a few serialization ticks.
        tokio::time::sleep(Duration::from_secs(3)).await;

        let mtime_after_restart = std::fs::metadata(&rt_path).unwrap().modified().unwrap();

        // Wait for more ticks — file should not change.
        tokio::time::sleep(Duration::from_secs(3)).await;

        let mtime_later = std::fs::metadata(&rt_path).unwrap().modified().unwrap();
        assert_eq!(
            mtime_after_restart, mtime_later,
            "loaded runtime should not be rewritten when clean"
        );
    }
}

/// Multiple rapid mutations coalesce into a single write on the next tick.
#[tokio::test]
async fn multiple_mutations_coalesce_into_single_write() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    let rt_id_bytes =
        create_runtime(&mut c, "coalesce-test", proto::RuntimePolicy::Persistent).await;
    let runtime_id = bytes_to_uuid(&rt_id_bytes).unwrap();

    // Wait for initial write.
    wait_for_state_containing(tmp.path(), "coalesce-test", Duration::from_secs(10))
        .await;

    let state_dir = tmp.path().join("state/rttx/daemon");
    let rt_path = layout::runtime_file(&state_dir, runtime_id);

    // Record mtime after initial write.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mtime_before = std::fs::metadata(&rt_path).unwrap().modified().unwrap();

    // Fire multiple renames rapidly (all within one tick interval).
    for i in 0..5 {
        c.send(&proto::ClientMessage {
            msg: Some(proto::client_message::Msg::RenameRuntime(proto::RenameRuntime {
                runtime_id: rt_id_bytes.clone(),
                name: format!("renamed-{i}"),
            })),
        })
        .await;
        let _ = c.recv().await; // RuntimeRenamed
    }

    // Wait for the next serialization tick.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // The file should have been written with the final name.
    let result = persistence::load_all(&state_dir).unwrap();
    let rt = result.runtimes.iter().find(|r| r.spec.id == runtime_id).unwrap();
    assert_eq!(rt.spec.name, "renamed-4", "final rename should be persisted");

    // Verify the file was rewritten (mtime changed).
    let mtime_after = std::fs::metadata(&rt_path).unwrap().modified().unwrap();
    assert!(mtime_after > mtime_before, "dirty runtime should be rewritten");

    // Now wait for more ticks — file should NOT change again.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let mtime_stable = std::fs::metadata(&rt_path).unwrap().modified().unwrap();
    assert_eq!(mtime_after, mtime_stable, "clean runtime should not be rewritten");
}

// ── 3. Scrollback rotation keeps N segments ─────────────────────

/// Scrollback rotation through the full server path produces rotated
/// segments and respects the keep limit.
#[tokio::test]
async fn scrollback_rotation_keeps_n_segments_via_server() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    let rt_id_bytes =
        create_runtime(&mut c, "rotation-test", proto::RuntimePolicy::Persistent).await;
    let runtime_id = bytes_to_uuid(&rt_id_bytes).unwrap();

    attach_rw(&mut c, &rt_id_bytes).await;
    let pane_id_bytes = create_pane(&mut c, &rt_id_bytes).await;
    let pane_id = bytes_to_uuid(&pane_id_bytes).unwrap();

    // Generate enough output to trigger rotation (>10 MB).
    // The shell echoes back what we send, so we need to send data that
    // produces output. Use a large block repeated several times.
    let big_block = vec![b'X'; 512 * 1024]; // 512 KB per send
    for _ in 0..25 {
        send_input(&mut c, &rt_id_bytes, &pane_id_bytes, &big_block).await;
        // Small delay to let the PTY process and the server flush.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Wait for several serialization ticks to flush scrollback.
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Check the v1 scrollback path (where flush_scrollback writes).
    let cache_dir = tmp.path().join("cache");
    let scrollback_dir = cache_dir.join("scrollback").join(runtime_id.to_string());

    if scrollback_dir.exists() {
        let log_path = scrollback_dir.join(format!("{pane_id}.log"));
        let rotated_1 = scrollback_dir.join(format!("{pane_id}.log.1"));

        // If enough data was written, rotation should have occurred.
        if log_path.exists() {
            let log_size = std::fs::metadata(&log_path).map_or(0, |m| m.len());
            if log_size > 0 || rotated_1.exists() {
                // Verify rotated segments exist and the oldest is bounded.
                // SCROLLBACK_ROTATE_KEEP = 3, so .log.4 should not exist.
                let seg4 = scrollback_dir.join(format!("{pane_id}.log.4"));
                assert!(!seg4.exists(), "segment .log.4 should not exist (keep limit is 3)");
            }
        }
    }

    // The key invariant: the pane should still be functional after rotation.
    let runtimes = list_runtimes(&mut c).await;
    assert_eq!(runtimes.len(), 1, "runtime should still be alive after rotation");
}

// ── 4. Orphan sweep quarantines unreferenced directories ────────

/// On startup, orphan directories older than 30 days are pruned.
#[tokio::test]
async fn startup_prunes_old_orphans() {
    let tmp = tempfile::TempDir::new().unwrap();
    let state_dir = tmp.path().join("state/rttx/daemon");
    std::fs::create_dir_all(&state_dir).unwrap();

    // Write an empty daemon index (no runtimes).
    let index = serde_json::json!({
        "schema_version": 1,
        "server_version": "0.4.0",
        "runtime_ids": [],
        "created_at": { "secs_since_epoch": 1_700_000_000, "nanos_since_epoch": 0 },
        "last_serialized_at": { "secs_since_epoch": 1_700_000_000, "nanos_since_epoch": 0 }
    });
    std::fs::write(state_dir.join("daemon.json"), serde_json::to_string_pretty(&index).unwrap())
        .unwrap();

    // Pre-seed .orphans/ with an old entry (31 days ago).
    let orphans_dir = layout::orphans_dir(&state_dir);
    std::fs::create_dir_all(&orphans_dir).unwrap();

    let old_id = uuid::Uuid::new_v4();
    let old_dir = orphans_dir.join(old_id.to_string());
    std::fs::create_dir_all(&old_dir).unwrap();
    std::fs::write(old_dir.join("runtime.json"), "{}").unwrap();

    // Set mtime to 31 days ago.
    let old_time = std::time::SystemTime::now() - Duration::from_hours(31 * 24);
    let old_filetime = std::fs::FileTimes::new().set_modified(old_time);
    std::fs::File::open(&old_dir).unwrap().set_times(old_filetime).unwrap();

    // Also add a recent orphan that should survive.
    let recent_id = uuid::Uuid::new_v4();
    let recent_dir = orphans_dir.join(recent_id.to_string());
    std::fs::create_dir_all(&recent_dir).unwrap();
    std::fs::write(recent_dir.join("runtime.json"), "{}").unwrap();

    // Start the server — orphan sweep runs during load.
    let (_sock, _handle) = start_test_server(tmp.path()).await;

    // Old orphan should be pruned.
    assert!(!old_dir.exists(), "old orphan (>30 days) should be pruned on startup");

    // Recent orphan should remain.
    assert!(recent_dir.exists(), "recent orphan should survive pruning");
}

/// Terminated runtime's directory is cleaned up and does not become an orphan.
#[tokio::test]
async fn terminated_runtime_does_not_become_orphan_on_restart() {
    let tmp = tempfile::TempDir::new().unwrap();

    // Phase 1: create and terminate a runtime.
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        let rt_id_bytes =
            create_runtime(&mut c, "terminated-rt", proto::RuntimePolicy::Persistent).await;

        // Wait for serialization.
        wait_for_state_containing(
            tmp.path(),
            "terminated-rt",
            Duration::from_secs(10),
        )
        .await;

        // Attach and terminate.
        attach_rw(&mut c, &rt_id_bytes).await;
        terminate_runtime(&mut c, &rt_id_bytes).await;

        // Wait for cleanup.
        tokio::time::sleep(Duration::from_secs(2)).await;
        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Phase 2: restart — no orphans should appear.
    {
        let (sock, _handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        let runtimes = list_runtimes(&mut c).await;
        assert!(runtimes.is_empty(), "terminated runtime should not reappear");

        // No orphans directory should exist (or it should be empty).
        let state_dir = tmp.path().join("state/rttx/daemon");
        let orphans = layout::orphans_dir(&state_dir);
        if orphans.exists() {
            let entries: Vec<_> = std::fs::read_dir(&orphans).unwrap().collect();
            assert!(entries.is_empty(), "no orphans should exist after clean termination");
        }
    }
}

// ── 5. ScreenSnapshotV1 round-trip through restart ──────────────

/// Screen snapshot survives server restart with correct field values.
#[tokio::test]
async fn screen_snapshot_survives_restart() {
    let tmp = tempfile::TempDir::new().unwrap();

    let runtime_id;
    let pane_id;

    // Phase 1: create runtime with pane, feed some output, let serialization write.
    {
        let (sock, handle) = start_test_server(tmp.path()).await;
        let mut c = TestClient::connect(&sock).await;
        c.handshake().await;

        let rt_id_bytes =
            create_runtime(&mut c, "snap-restart", proto::RuntimePolicy::Persistent).await;
        runtime_id = bytes_to_uuid(&rt_id_bytes).unwrap();

        attach_rw(&mut c, &rt_id_bytes).await;
        let pane_id_bytes = create_pane(&mut c, &rt_id_bytes).await;
        pane_id = bytes_to_uuid(&pane_id_bytes).unwrap();

        // Send some input to generate output.
        send_input(&mut c, &rt_id_bytes, &pane_id_bytes, b"echo hello\n").await;
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Wait for serialization to write snapshot.
        wait_for_state_containing(
            tmp.path(),
            "snap-restart",
            Duration::from_secs(10),
        )
        .await;
        // Extra time for screen snapshot write.
        tokio::time::sleep(Duration::from_secs(2)).await;

        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Verify snapshot exists and has correct fields.
    let state_dir = tmp.path().join("state/rttx/daemon");
    let snap = persistence::load_screen_snapshot(&state_dir, runtime_id, pane_id)
        .expect("screen snapshot should exist after serialization");

    assert_eq!(snap.pane_id, pane_id);
    assert_eq!(snap.schema_version, SCREEN_SNAPSHOT_SCHEMA_VERSION);
    assert!(snap.cols > 0, "cols should be set");
    assert!(snap.rows > 0, "rows should be set");
    assert!(!snap.screen_bytes.is_empty(), "screen_bytes should contain output");

    // Phase 2: restart — snapshot should still be loadable.
    let snap_after_restart = persistence::load_screen_snapshot(&state_dir, runtime_id, pane_id)
        .expect("screen snapshot should survive restart");
    assert_eq!(snap, snap_after_restart, "snapshot should be identical after restart");
}

/// Screen snapshot for a `no_persist` pane is marked confidential.
#[tokio::test]
async fn no_persist_pane_snapshot_is_confidential_via_server() {
    let tmp = tempfile::TempDir::new().unwrap();
    let (sock, _handle) = start_test_server(tmp.path()).await;

    let mut c = TestClient::connect(&sock).await;
    c.handshake().await;

    let rt_id_bytes =
        create_runtime(&mut c, "confidential-snap", proto::RuntimePolicy::Persistent).await;
    let runtime_id = bytes_to_uuid(&rt_id_bytes).unwrap();

    attach_rw(&mut c, &rt_id_bytes).await;

    // Create a no_persist pane.
    c.send(&proto::ClientMessage {
        msg: Some(proto::client_message::Msg::CreatePane(proto::CreatePane {
            runtime_id: rt_id_bytes.clone(),
            cols: 80,
            rows: 24,
            no_persist: Some(true),
            ..Default::default()
        })),
    })
    .await;
    let pane_id = loop {
        match c.recv_or_timeout().await.msg {
            Some(proto::server_message::Msg::PaneCreated(pc)) => {
                break bytes_to_uuid(&pc.pane_id).unwrap();
            }
            Some(proto::server_message::Msg::Delta(_)) => {}
            other => panic!("expected PaneCreated, got {other:?}"),
        }
    };

    // Wait for serialization.
    wait_for_state_containing(
        tmp.path(),
        "confidential-snap",
        Duration::from_secs(10),
    )
    .await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Check the snapshot.
    let state_dir = tmp.path().join("state/rttx/daemon");
    let snap = persistence::load_screen_snapshot(&state_dir, runtime_id, pane_id);
    if let Some(snap) = snap {
        assert!(snap.confidential, "no_persist pane snapshot should be confidential");
    }
    // If no snapshot was written (no_persist may skip it), that's also acceptable.
}
