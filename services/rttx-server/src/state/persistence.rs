//! High-level persistence operations for per-workspace state (RFC-031 §6).
//!
//! Provides `load_all` and `save_workspace` / `save_daemon_index` that use
//! the layout paths, typed structs, and atomic I/O.
//!
//! Loading reads exactly one schema version — the current one. A file with any
//! other `schema_version` (or that fails to parse) is treated as unsupported
//! and skipped; there is no migration path and no special-cased reset.

use crate::state::io::write_with_backup;
use crate::state::layout;
use crate::state::migrations::peek_schema_version;
use crate::state::types::{
    DAEMON_INDEX_SCHEMA_VERSION, DaemonIndexV1, RUNTIME_FILE_SCHEMA_VERSION, ScreenSnapshotV1,
    WorkspaceFileV2,
};
use std::path::Path;
use std::time::SystemTime;
use uuid::Uuid;

/// Result of loading all persisted state on startup.
#[derive(Debug)]
pub struct LoadResult {
    /// Successfully loaded workspace files.
    pub workspaces: Vec<WorkspaceFileV2>,
    /// Workspace IDs that failed to load (corrupt, unreadable, or an
    /// unsupported schema version) and were skipped.
    pub failed_ids: Vec<Uuid>,
}

/// Load all persisted state from the daemon state directory.
///
/// Returns `None` if no `daemon.json` exists (first startup). Workspaces that
/// are corrupt or carry an unsupported schema version are skipped; this is not
/// fatal.
pub fn load_all(state_dir: &Path) -> Option<LoadResult> {
    let index_path = layout::daemon_index(state_dir);

    // Try primary, then backup on parse failure.
    let index = load_daemon_index_with_fallback(&index_path)?;

    let mut workspaces = Vec::new();
    let mut failed_ids = Vec::new();

    for &runtime_id in &index.runtime_ids {
        let short = &runtime_id.to_string()[..8];
        match load_workspace(state_dir, runtime_id) {
            WorkspaceLoad::Loaded(wf) => workspaces.push(*wf),
            WorkspaceLoad::Corrupt => {
                tracing::error!("Failed to load workspace {short} — skipping");
                failed_ids.push(runtime_id);
            }
            WorkspaceLoad::NotFound => {
                tracing::error!("Workspace file not found for {short} — skipping");
                failed_ids.push(runtime_id);
            }
        }
    }

    Some(LoadResult { workspaces, failed_ids })
}

/// Load daemon index, trying backup on parse failure.
fn load_daemon_index_with_fallback(path: &Path) -> Option<DaemonIndexV1> {
    use crate::state::io::{read_backup, read_primary};

    if let Some(json) = read_primary(path) {
        match crate::state::migrations::load_daemon_index(&json) {
            Ok(idx) => return Some(idx),
            Err(e) => {
                tracing::warn!("Primary daemon index corrupt: {e} — trying backup");
            }
        }
    }

    if let Some(json) = read_backup(path) {
        match crate::state::migrations::load_daemon_index(&json) {
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

/// Outcome of attempting to load a single workspace file from disk.
enum WorkspaceLoad {
    /// A current-schema workspace file loaded successfully.
    Loaded(Box<WorkspaceFileV2>),
    /// Unreadable, unparseable, or any non-current schema version; skip.
    Corrupt,
    /// No workspace file present at primary or backup.
    NotFound,
}

/// Classify a workspace JSON document by its schema version. Only the current
/// schema loads; any other version (older or newer) or a parse failure is
/// unsupported and treated as corrupt.
fn classify_workspace_json(json: &str) -> WorkspaceLoad {
    let Ok(version) = peek_schema_version(json) else {
        return WorkspaceLoad::Corrupt;
    };
    if version == RUNTIME_FILE_SCHEMA_VERSION {
        serde_json::from_str::<WorkspaceFileV2>(json)
            .map_or(WorkspaceLoad::Corrupt, |wf| WorkspaceLoad::Loaded(Box::new(wf)))
    } else {
        WorkspaceLoad::Corrupt
    }
}

/// Load a single workspace file, trying backup when the primary is corrupt.
fn load_workspace(state_dir: &Path, runtime_id: Uuid) -> WorkspaceLoad {
    let path = layout::runtime_file(state_dir, runtime_id);

    if let Some(json) = crate::state::io::read_primary(&path) {
        match classify_workspace_json(&json) {
            WorkspaceLoad::Corrupt => {
                tracing::warn!(
                    "Primary workspace file corrupt for {} — trying backup",
                    &runtime_id.to_string()[..8]
                );
            }
            recognized => return recognized,
        }
    }

    if let Some(json) = crate::state::io::read_backup(&path) {
        let outcome = classify_workspace_json(&json);
        if matches!(outcome, WorkspaceLoad::Loaded(_)) {
            tracing::info!("Recovered workspace {} from backup", &runtime_id.to_string()[..8]);
        }
        return outcome;
    }

    WorkspaceLoad::NotFound
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

/// Save a single workspace file to disk with backup.
pub fn save_workspace(state_dir: &Path, runtime_file: &WorkspaceFileV2) -> std::io::Result<()> {
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
    // Atomic write: temp file + rename prevents corrupt snapshots if
    // the daemon is killed mid-write.
    let tmp_path = path.with_extension("snap.tmp");
    std::fs::write(&tmp_path, &json)?;
    std::fs::rename(&tmp_path, &path)?;
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

/// Remove a workspace's directory from disk.
pub fn remove_runtime_dir(state_dir: &Path, runtime_id: Uuid) -> std::io::Result<()> {
    let dir = layout::runtime_dir(state_dir, runtime_id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane_tree::{PaneId, SplitAxis, WorkspaceTree};
    use crate::state::types::{
        PaneSpecV2, RUNTIME_FILE_SCHEMA_VERSION, SCREEN_SNAPSHOT_SCHEMA_VERSION, ScreenSnapshotV1,
        TerminalModeSnapshot, WorkspaceFileV2, WorkspaceInstanceV1, WorkspaceSpecV2,
    };
    use crate::workspace::WorkspacePolicy;
    use tempfile::TempDir;

    fn sample_runtime_file(id: Uuid) -> WorkspaceFileV2 {
        let pane_id = PaneId::new();
        let mut tree = WorkspaceTree::new();
        tree.insert_root(pane_id);
        WorkspaceFileV2 {
            schema_version: RUNTIME_FILE_SCHEMA_VERSION,
            spec: WorkspaceSpecV2 {
                id,
                name: "test-rt".into(),
                policy: WorkspacePolicy::Persistent,
                created_at: SystemTime::now(),
                tree,
                panes: vec![PaneSpecV2 {
                    id: pane_id,
                    cwd: Some("/home/user".into()),
                    title: Some("bash".into()),
                    exit_status: None,
                    cols: 80,
                    rows: 24,
                    no_persist: false,
                }],
            },
            instance: WorkspaceInstanceV1 {
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
        save_workspace(state_dir, &rf).unwrap();

        let result = load_all(state_dir).unwrap();
        assert!(result.failed_ids.is_empty());
        assert_eq!(result.workspaces.len(), 1);
        assert_eq!(result.workspaces[0].spec.id, rt_id);
        assert_eq!(result.workspaces[0].spec.name, "test-rt");
    }

    /// A multi-pane tree (structure + ratios + default-active) survives a
    /// save/load round trip through the persistence layer (RFC-031 §6).
    #[test]
    fn save_and_load_preserves_multi_pane_tree() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let rt_id = Uuid::new_v4();

        let (a, b, c) = (PaneId::new(), PaneId::new(), PaneId::new());
        let mut tree = WorkspaceTree::new();
        tree.insert_root(a);
        tree.split(a, b, SplitAxis::Horizontal, 0.25);
        tree.split(b, c, SplitAxis::Vertical, 0.75);
        tree.set_default_active(c);
        let expected_tree = tree.clone();

        let mk = |id: PaneId| PaneSpecV2 {
            id,
            cwd: None,
            title: None,
            exit_status: None,
            cols: 80,
            rows: 24,
            no_persist: false,
        };
        let rf = WorkspaceFileV2 {
            schema_version: RUNTIME_FILE_SCHEMA_VERSION,
            spec: WorkspaceSpecV2 {
                id: rt_id,
                name: "tree-ws".into(),
                policy: WorkspacePolicy::Persistent,
                created_at: SystemTime::now(),
                tree,
                panes: vec![mk(a), mk(b), mk(c)],
            },
            instance: WorkspaceInstanceV1 {
                revision: 9,
                last_active_at: SystemTime::now(),
                last_snapshot_at: SystemTime::now(),
            },
        };

        save_daemon_index(state_dir, &[rt_id]).unwrap();
        save_workspace(state_dir, &rf).unwrap();

        let result = load_all(state_dir).unwrap();
        assert_eq!(result.workspaces.len(), 1);
        let loaded = &result.workspaces[0];
        assert_eq!(loaded.spec.tree, expected_tree, "tree must round-trip exactly");
        assert_eq!(loaded.spec.tree.default_active(), Some(c));
        assert_eq!(loaded.spec.panes.len(), 3);
    }

    /// A workspace file whose `schema_version` is not the current one is skipped
    /// on load — there is no migration path.
    #[test]
    fn unsupported_schema_workspace_is_skipped() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let old_id = Uuid::new_v4();

        // A workspace file carrying a non-current schema version must not load.
        let old_dir = layout::runtime_dir(state_dir, old_id);
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(
            layout::runtime_file(state_dir, old_id),
            r#"{"schema_version": 99, "spec": {}, "instance": {}}"#,
        )
        .unwrap();
        save_daemon_index(state_dir, &[old_id]).unwrap();

        let result = load_all(state_dir).unwrap();
        assert!(result.workspaces.is_empty(), "unsupported-schema workspace must not load");
        assert_eq!(result.failed_ids, vec![old_id], "it is skipped like any unsupported file");
    }

    /// A current workspace still loads even when a sibling carries an
    /// unsupported schema version.
    #[test]
    fn unsupported_schema_does_not_drop_current_workspaces() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let good_id = Uuid::new_v4();
        let old_id = Uuid::new_v4();

        save_workspace(state_dir, &sample_runtime_file(good_id)).unwrap();

        let old_dir = layout::runtime_dir(state_dir, old_id);
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(
            layout::runtime_file(state_dir, old_id),
            r#"{"schema_version": 99, "spec": {}, "instance": {}}"#,
        )
        .unwrap();

        save_daemon_index(state_dir, &[good_id, old_id]).unwrap();

        let result = load_all(state_dir).unwrap();
        assert_eq!(result.workspaces.len(), 1);
        assert_eq!(result.workspaces[0].spec.id, good_id);
        assert_eq!(result.failed_ids, vec![old_id]);
    }

    #[test]
    fn newer_schema_version_is_skipped_not_loaded() {
        // Forward-compat: a file whose schema_version is *newer* than this
        // daemon understands is treated the same as any unsupported version —
        // skipped, never loaded or reset.
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let future_id = Uuid::new_v4();

        let dir = layout::runtime_dir(state_dir, future_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            layout::runtime_file(state_dir, future_id),
            r#"{"schema_version": 9999, "spec": {}, "instance": {}}"#,
        )
        .unwrap();
        save_daemon_index(state_dir, &[future_id]).unwrap();

        let result = load_all(state_dir).unwrap();
        assert!(result.workspaces.is_empty(), "an unsupported future schema must not load");
        assert_eq!(result.failed_ids, vec![future_id]);
    }

    #[test]
    fn load_all_returns_none_when_no_index() {
        let tmp = TempDir::new().unwrap();
        let result = load_all(tmp.path());
        assert!(result.is_none());
    }

    #[test]
    fn corrupt_workspace_is_skipped_not_fatal() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let good_id = Uuid::new_v4();
        let bad_id = Uuid::new_v4();

        // Save good workspace
        let rf = sample_runtime_file(good_id);
        save_workspace(state_dir, &rf).unwrap();

        // Write corrupt workspace file
        let bad_dir = layout::runtime_dir(state_dir, bad_id);
        std::fs::create_dir_all(&bad_dir).unwrap();
        std::fs::write(layout::runtime_file(state_dir, bad_id), "not valid json").unwrap();

        // Save index referencing both
        save_daemon_index(state_dir, &[good_id, bad_id]).unwrap();

        let result = load_all(state_dir).unwrap();
        assert_eq!(result.workspaces.len(), 1);
        assert_eq!(result.workspaces[0].spec.id, good_id);
        assert_eq!(result.failed_ids, vec![bad_id]);
    }

    #[test]
    fn multiple_workspaces_round_trip() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();

        for &id in &ids {
            save_workspace(state_dir, &sample_runtime_file(id)).unwrap();
        }
        save_daemon_index(state_dir, &ids).unwrap();

        let result = load_all(state_dir).unwrap();
        assert_eq!(result.workspaces.len(), 3);
        assert!(result.failed_ids.is_empty());
    }

    #[test]
    fn remove_runtime_dir_cleans_up() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let rt_id = Uuid::new_v4();

        save_workspace(state_dir, &sample_runtime_file(rt_id)).unwrap();
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
    fn save_workspace_overwrites_with_backup() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let rt_id = Uuid::new_v4();

        let mut rf = sample_runtime_file(rt_id);
        rf.spec.name = "version-1".into();
        save_workspace(state_dir, &rf).unwrap();

        rf.spec.name = "version-2".into();
        save_workspace(state_dir, &rf).unwrap();

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
        save_workspace(state_dir, &rf).unwrap();

        // Corrupt the primary
        let path = layout::runtime_file(state_dir, rt_id);
        let prev = path.with_extension("prev");
        let bak = path.with_extension("bak");

        // Copy good to prev, corrupt primary
        std::fs::copy(&path, &prev).unwrap();
        let _ = std::fs::remove_file(&bak);
        std::os::unix::fs::symlink("workspace.prev", &bak).unwrap();
        std::fs::write(&path, "corrupted!").unwrap();

        // Save index
        save_daemon_index(state_dir, &[rt_id]).unwrap();

        let result = load_all(state_dir).unwrap();
        assert_eq!(result.workspaces.len(), 1);
        assert_eq!(result.workspaces[0].spec.id, rt_id);
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
                alternate_screen: false,
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

    #[test]
    fn load_all_with_empty_index_yields_no_workspaces() {
        let tmp = TempDir::new().unwrap();
        save_daemon_index(tmp.path(), &[]).unwrap();

        let result = load_all(tmp.path()).expect("an empty state dir loads");
        assert!(result.workspaces.is_empty());
        assert!(result.failed_ids.is_empty());
    }
}
