//! High-level persistence operations for v2 per-runtime state (RFC-022 §3, §6).
//!
//! Provides `load_all` and `save_runtime` / `save_daemon_index` that use
//! the layout paths, typed structs, migration chain, and atomic I/O.

use crate::state::io::write_with_backup;
use crate::state::layout;
use crate::state::migrations::{self, MigrationError};
use crate::state::types::{
    DAEMON_INDEX_SCHEMA_VERSION, DaemonIndexV1, RuntimeFileV1, ScreenSnapshotV1,
};
use std::path::Path;
use std::time::SystemTime;
use uuid::Uuid;

/// Result of loading all persisted v2 state on startup.
#[derive(Debug)]
pub struct LoadResult {
    /// Successfully loaded runtime files.
    pub runtimes: Vec<RuntimeFileV1>,
    /// Runtime IDs that failed to load (corrupt or unreadable).
    pub failed_ids: Vec<Uuid>,
}

/// Load all v2 state from the daemon state directory.
///
/// Returns `None` if no `daemon.json` exists (first v2 startup).
/// Individual corrupt runtimes are logged and skipped, not fatal.
pub fn load_all(state_dir: &Path) -> Option<LoadResult> {
    let index_path = layout::daemon_index(state_dir);

    // Try primary, then backup on parse failure.
    let index = load_daemon_index_with_fallback(&index_path)?;

    let mut runtimes = Vec::new();
    let mut failed_ids = Vec::new();

    for &runtime_id in &index.runtime_ids {
        match load_runtime(state_dir, runtime_id) {
            Ok(rf) => runtimes.push(rf),
            Err(e) => {
                tracing::error!(
                    "Failed to load runtime {}: {e} — skipping",
                    &runtime_id.to_string()[..8]
                );
                failed_ids.push(runtime_id);
            }
        }
    }

    Some(LoadResult { runtimes, failed_ids })
}

/// Load daemon index, trying backup on parse failure.
fn load_daemon_index_with_fallback(path: &Path) -> Option<DaemonIndexV1> {
    use crate::state::io::{read_backup, read_primary};

    if let Some(json) = read_primary(path) {
        match migrations::load_daemon_index(&json) {
            Ok(idx) => return Some(idx),
            Err(e) => {
                tracing::warn!("Primary daemon index corrupt: {e} — trying backup");
            }
        }
    }

    if let Some(json) = read_backup(path) {
        match migrations::load_daemon_index(&json) {
            Ok(idx) => {
                tracing::info!("Recovered daemon index from backup");
                return Some(idx);
            }
            Err(e) => {
                tracing::error!("Backup daemon index also corrupt: {e}");
            }
        }
    }

    // Check if the file simply doesn't exist (first startup)
    if !path.exists() && !path.with_extension("prev").exists() {
        return None;
    }

    tracing::error!("Daemon index unreadable from both primary and backup");
    None
}

/// Load a single runtime file, trying backup on parse failure.
fn load_runtime(state_dir: &Path, runtime_id: Uuid) -> Result<RuntimeFileV1, LoadError> {
    let path = layout::runtime_file(state_dir, runtime_id);

    // Try primary first
    if let Some(json) = crate::state::io::read_primary(&path) {
        match migrations::load_runtime_file(&json) {
            Ok(rf) => return Ok(rf),
            Err(e) => {
                tracing::warn!(
                    "Primary runtime file corrupt for {}: {e} — trying backup",
                    &runtime_id.to_string()[..8]
                );
            }
        }
    }

    // Try backup
    if let Some(json) = crate::state::io::read_backup(&path) {
        match migrations::load_runtime_file(&json) {
            Ok(rf) => {
                tracing::info!("Recovered runtime {} from backup", &runtime_id.to_string()[..8]);
                return Ok(rf);
            }
            Err(e) => {
                return Err(LoadError::Migration(e));
            }
        }
    }

    Err(LoadError::NotFound(runtime_id))
}

/// Save the daemon index to disk with backup.
pub fn save_daemon_index(state_dir: &Path, runtime_ids: &[Uuid]) -> std::io::Result<()> {
    let index = DaemonIndexV1 {
        schema_version: DAEMON_INDEX_SCHEMA_VERSION,
        server_version: env!("CARGO_PKG_VERSION").to_string(),
        runtime_ids: runtime_ids.to_vec(),
        created_at: SystemTime::now(),
        last_serialized_at: SystemTime::now(),
    };
    let json = serde_json::to_string_pretty(&index).map_err(std::io::Error::other)?;
    write_with_backup(&layout::daemon_index(state_dir), &json)
}

/// Save a single runtime file to disk with backup.
pub fn save_runtime(state_dir: &Path, runtime_file: &RuntimeFileV1) -> std::io::Result<()> {
    let path = layout::runtime_file(state_dir, runtime_file.spec.id);
    let json = serde_json::to_string_pretty(runtime_file).map_err(std::io::Error::other)?;
    write_with_backup(&path, &json)
}

/// Save a screen snapshot for a pane to disk.
pub fn save_screen_snapshot(
    state_dir: &Path,
    runtime_id: Uuid,
    snapshot: &ScreenSnapshotV1,
) -> std::io::Result<()> {
    let path = layout::screen_snapshot(state_dir, runtime_id, snapshot.pane_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string(snapshot).map_err(std::io::Error::other)?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// Load a screen snapshot for a pane from disk.
///
/// Returns `None` if the file does not exist or is corrupt. A corrupt
/// snapshot is not fatal — the pane resurrects with a blank screen.
pub fn load_screen_snapshot(
    state_dir: &Path,
    runtime_id: Uuid,
    pane_id: Uuid,
) -> Option<ScreenSnapshotV1> {
    let path = layout::screen_snapshot(state_dir, runtime_id, pane_id);
    let json = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<ScreenSnapshotV1>(&json) {
        Ok(snap) => Some(snap),
        Err(e) => {
            tracing::warn!(
                "Corrupt screen snapshot for pane {}: {e} — starting with blank screen",
                &pane_id.to_string()[..8]
            );
            None
        }
    }
}

/// Remove a runtime's directory from disk.
pub fn remove_runtime_dir(state_dir: &Path, runtime_id: Uuid) -> std::io::Result<()> {
    let dir = layout::runtime_dir(state_dir, runtime_id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

/// Errors that can occur when loading a single runtime.
#[derive(Debug)]
enum LoadError {
    NotFound(Uuid),
    Migration(MigrationError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "runtime file not found for {id}"),
            Self::Migration(e) => write!(f, "{e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimePolicy;
    use crate::state::types::{
        HistoryEntryV1, PaneSpecV1, RUNTIME_FILE_SCHEMA_VERSION, RuntimeFileV1, RuntimeInstanceV1,
        RuntimeSpecV1, SCREEN_SNAPSHOT_SCHEMA_VERSION, ScreenSnapshotV1, TerminalModeSnapshot,
    };
    use tempfile::TempDir;

    fn sample_runtime_file(id: Uuid) -> RuntimeFileV1 {
        RuntimeFileV1 {
            schema_version: RUNTIME_FILE_SCHEMA_VERSION,
            spec: RuntimeSpecV1 {
                id,
                name: "test-rt".into(),
                policy: RuntimePolicy::Persistent,
                created_at: SystemTime::now(),
                panes: vec![PaneSpecV1 {
                    id: Uuid::new_v4(),
                    cwd: Some("/home/user".into()),
                    title: Some("bash".into()),
                    exit_status: None,
                    cols: 80,
                    rows: 24,
                    no_persist: false,
                }],
                active_pane_id: None,
                command_history: vec![HistoryEntryV1 {
                    command: "ls".into(),
                    cwd: "/".into(),
                    timestamp: SystemTime::now(),
                    pane_id: Uuid::new_v4(),
                }],
            },
            instance: RuntimeInstanceV1 {
                revision: 5,
                last_active_at: SystemTime::now(),
                last_snapshot_at: SystemTime::now(),
            },
        }
    }

    #[test]
    fn save_and_load_round_trip() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let rt_id = Uuid::new_v4();
        let rf = sample_runtime_file(rt_id);

        save_daemon_index(state_dir, &[rt_id]).unwrap();
        save_runtime(state_dir, &rf).unwrap();

        let result = load_all(state_dir).unwrap();
        assert!(result.failed_ids.is_empty());
        assert_eq!(result.runtimes.len(), 1);
        assert_eq!(result.runtimes[0].spec.id, rt_id);
        assert_eq!(result.runtimes[0].spec.name, "test-rt");
    }

    #[test]
    fn load_all_returns_none_when_no_index() {
        let tmp = TempDir::new().unwrap();
        let result = load_all(tmp.path());
        assert!(result.is_none());
    }

    #[test]
    fn corrupt_runtime_is_skipped_not_fatal() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let good_id = Uuid::new_v4();
        let bad_id = Uuid::new_v4();

        // Save good runtime
        let rf = sample_runtime_file(good_id);
        save_runtime(state_dir, &rf).unwrap();

        // Write corrupt runtime file
        let bad_dir = layout::runtime_dir(state_dir, bad_id);
        std::fs::create_dir_all(&bad_dir).unwrap();
        std::fs::write(layout::runtime_file(state_dir, bad_id), "not valid json").unwrap();

        // Save index referencing both
        save_daemon_index(state_dir, &[good_id, bad_id]).unwrap();

        let result = load_all(state_dir).unwrap();
        assert_eq!(result.runtimes.len(), 1);
        assert_eq!(result.runtimes[0].spec.id, good_id);
        assert_eq!(result.failed_ids, vec![bad_id]);
    }

    #[test]
    fn multiple_runtimes_round_trip() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();

        for &id in &ids {
            save_runtime(state_dir, &sample_runtime_file(id)).unwrap();
        }
        save_daemon_index(state_dir, &ids).unwrap();

        let result = load_all(state_dir).unwrap();
        assert_eq!(result.runtimes.len(), 3);
        assert!(result.failed_ids.is_empty());
    }

    #[test]
    fn remove_runtime_dir_cleans_up() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let rt_id = Uuid::new_v4();

        save_runtime(state_dir, &sample_runtime_file(rt_id)).unwrap();
        let dir = layout::runtime_dir(state_dir, rt_id);
        assert!(dir.exists());

        remove_runtime_dir(state_dir, rt_id).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn remove_nonexistent_runtime_dir_is_ok() {
        let tmp = TempDir::new().unwrap();
        let result = remove_runtime_dir(tmp.path(), Uuid::new_v4());
        assert!(result.is_ok());
    }

    #[test]
    fn save_runtime_overwrites_with_backup() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let rt_id = Uuid::new_v4();

        let mut rf = sample_runtime_file(rt_id);
        rf.spec.name = "version-1".into();
        save_runtime(state_dir, &rf).unwrap();

        rf.spec.name = "version-2".into();
        save_runtime(state_dir, &rf).unwrap();

        // Primary has v2
        let path = layout::runtime_file(state_dir, rt_id);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("version-2"));

        // Backup has v1
        let prev = path.with_extension("prev");
        let backup_content = std::fs::read_to_string(&prev).unwrap();
        assert!(backup_content.contains("version-1"));
    }

    #[test]
    fn load_recovers_from_backup_when_primary_corrupt() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let rt_id = Uuid::new_v4();

        // Write a good version first
        let rf = sample_runtime_file(rt_id);
        save_runtime(state_dir, &rf).unwrap();

        // Corrupt the primary
        let path = layout::runtime_file(state_dir, rt_id);
        let prev = path.with_extension("prev");
        let bak = path.with_extension("bak");

        // Copy good to prev, corrupt primary
        std::fs::copy(&path, &prev).unwrap();
        let _ = std::fs::remove_file(&bak);
        std::os::unix::fs::symlink("runtime.prev", &bak).unwrap();
        std::fs::write(&path, "corrupted!").unwrap();

        // Save index
        save_daemon_index(state_dir, &[rt_id]).unwrap();

        let result = load_all(state_dir).unwrap();
        assert_eq!(result.runtimes.len(), 1);
        assert_eq!(result.runtimes[0].spec.id, rt_id);
    }

    fn sample_screen_snapshot(pane_id: Uuid) -> ScreenSnapshotV1 {
        ScreenSnapshotV1 {
            schema_version: SCREEN_SNAPSHOT_SCHEMA_VERSION,
            pane_id,
            cols: 80,
            rows: 24,
            cursor_row: 5,
            cursor_col: 10,
            cursor_visible: true,
            title: Some("bash".into()),
            cwd: Some("/home/user".into()),
            pane_output_seq: 42,
            modes: TerminalModeSnapshot {
                bracketed_paste: true,
                application_cursor_keys: false,
                application_keypad: false,
                mouse_tracking_mode: 0,
                sgr_mouse: false,
                focus_reporting: false,
            },
            screen_bytes: b"hello world\r\n$ ".to_vec(),
            confidential: false,
        }
    }

    #[test]
    fn screen_snapshot_save_and_load_round_trip() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let rt_id = Uuid::new_v4();
        let pane_id = Uuid::new_v4();
        let snap = sample_screen_snapshot(pane_id);

        save_screen_snapshot(state_dir, rt_id, &snap).unwrap();
        let loaded = load_screen_snapshot(state_dir, rt_id, pane_id).unwrap();
        assert_eq!(snap, loaded);
    }

    #[test]
    fn screen_snapshot_load_returns_none_when_missing() {
        let tmp = TempDir::new().unwrap();
        let result = load_screen_snapshot(tmp.path(), Uuid::new_v4(), Uuid::new_v4());
        assert!(result.is_none());
    }

    #[test]
    fn screen_snapshot_load_returns_none_when_corrupt() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let rt_id = Uuid::new_v4();
        let pane_id = Uuid::new_v4();

        let path = layout::screen_snapshot(state_dir, rt_id, pane_id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not valid json").unwrap();

        let result = load_screen_snapshot(state_dir, rt_id, pane_id);
        assert!(result.is_none());
    }
}
