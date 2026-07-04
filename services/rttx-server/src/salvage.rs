//! One-time orphaned-histfile salvage utility (RFC-031 §9 / Step 7).
//!
//! Standalone, opt-in recovery for shell-history files left unreferenced by the
//! earlier random-pane-id bug, where durable state keyed on a
//! process-ephemeral pane id was silently orphaned when the id changed.
//!
//! This is **not** a daemon workspace code path. It is invoked only by the
//! `rttx-server salvage-history` subcommand. It performs a read-only scan of the
//! daemon state directory and copies orphaned history into a *separate* recovery
//! directory. It never mutates, removes, or rewrites live workspace state, so it
//! cannot interfere with a running daemon and reintroduces no compatibility code
//! into normal operation.

use crate::pane_tree::PaneId;
use crate::state::layout;
use crate::state::migrations::peek_schema_version;
use crate::state::types::{RUNTIME_FILE_SCHEMA_VERSION, WorkspaceFileV2};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// An orphaned per-pane shell-history file: present on disk under a workspace's
/// `history/` directory but not referenced by that workspace's current pane tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrphanHistfile {
    /// Workspace directory the file was found under.
    pub runtime_id: Uuid,
    /// Pane id encoded in the file name (`<pane_id>.hist`).
    pub pane_id: Uuid,
    /// Absolute path to the orphaned history file.
    pub path: PathBuf,
    /// File size in bytes.
    pub bytes: u64,
}

/// Outcome of an export run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SalvageReport {
    /// The orphans that were copied into the recovery directory.
    pub exported: Vec<OrphanHistfile>,
    /// Recovery directory the files were written to.
    pub dest: PathBuf,
    /// Total bytes copied.
    pub total_bytes: u64,
}

/// Scan the daemon state directory for orphaned history files.
///
/// A `history/<pane_id>.hist` file is orphaned when its `<pane_id>` is not
/// referenced by the workspace's current-schema pane tree. Workspaces whose
/// `workspace.json` is an unsupported older version, is corrupt, or references
/// no panes have all of their history files reported. Empty files are skipped —
/// there is nothing to recover.
///
/// The scan is strictly read-only: it never removes unsupported older-version workspaces the way
/// the daemon's clean-break loader does.
#[must_use]
pub fn scan_orphans(state_dir: &Path) -> Vec<OrphanHistfile> {
    let mut orphans = Vec::new();

    let Ok(entries) = std::fs::read_dir(layout::runtimes_dir(state_dir)) else {
        return orphans;
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(runtime_id) = Uuid::parse_str(name) else { continue };

        let referenced = referenced_pane_ids(state_dir, runtime_id);
        collect_workspace_orphans(state_dir, runtime_id, &referenced, &mut orphans);
    }

    orphans.sort_by_key(|o| (o.runtime_id, o.pane_id));
    orphans
}

/// Copy orphaned history files into a recovery directory, preserving provenance
/// as `<dest>/<runtime_id>/<pane_id>.hist`.
///
/// The destination is expected to live outside the daemon's `workspaces/` tree so
/// the live workspace path is never touched.
///
/// # Errors
///
/// Returns the first I/O error encountered while creating directories or copying
/// a file.
pub fn export_orphans(
    orphans: &[OrphanHistfile],
    dest_dir: &Path,
) -> std::io::Result<SalvageReport> {
    let mut exported = Vec::new();
    let mut total_bytes = 0u64;

    for orphan in orphans {
        let runtime_dir = dest_dir.join(orphan.runtime_id.to_string());
        std::fs::create_dir_all(&runtime_dir)?;
        let target = runtime_dir.join(format!("{}.hist", orphan.pane_id));
        total_bytes += std::fs::copy(&orphan.path, &target)?;
        exported.push(orphan.clone());
    }

    Ok(SalvageReport { exported, dest: dest_dir.to_path_buf(), total_bytes })
}

/// Pane ids referenced by a workspace's current-schema tree. Empty when the
/// workspace file is absent, corrupt, or an older schema (clean-break) — in which
/// case every history file under that workspace is considered orphaned.
fn referenced_pane_ids(state_dir: &Path, runtime_id: Uuid) -> BTreeSet<Uuid> {
    let path = layout::runtime_file(state_dir, runtime_id);
    let Ok(json) = std::fs::read_to_string(&path) else {
        return BTreeSet::new();
    };
    if peek_schema_version(&json).ok() != Some(RUNTIME_FILE_SCHEMA_VERSION) {
        return BTreeSet::new();
    }
    let Ok(workspace) = serde_json::from_str::<WorkspaceFileV2>(&json) else {
        return BTreeSet::new();
    };
    workspace
        .spec
        .tree
        .panes()
        .into_iter()
        .map(PaneId::uuid)
        .chain(workspace.spec.panes.iter().map(|p| p.id.uuid()))
        .collect()
}

/// Append every non-empty, unreferenced `*.hist` file under one workspace to `out`.
fn collect_workspace_orphans(
    state_dir: &Path,
    runtime_id: Uuid,
    referenced: &BTreeSet<Uuid>,
    out: &mut Vec<OrphanHistfile>,
) {
    let history_dir = layout::runtime_dir(state_dir, runtime_id).join("history");
    let Ok(files) = std::fs::read_dir(&history_dir) else { return };

    for file in files.flatten() {
        let path = file.path();
        if path.extension().and_then(|e| e.to_str()) != Some("hist") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        let Ok(pane_id) = Uuid::parse_str(stem) else { continue };
        if referenced.contains(&pane_id) {
            continue;
        }
        let bytes = file.metadata().map(|m| m.len()).unwrap_or_default();
        if bytes == 0 {
            continue;
        }
        out.push(OrphanHistfile { runtime_id, pane_id, path, bytes });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane_tree::WorkspaceTree;
    use crate::state::persistence::{save_daemon_index, save_workspace};
    use crate::state::types::{
        PaneSpecV2, RUNTIME_FILE_SCHEMA_VERSION, WorkspaceFileV2, WorkspaceInstanceV1,
        WorkspaceSpecV2,
    };
    use crate::workspace::WorkspacePolicy;
    use std::time::SystemTime;
    use tempfile::TempDir;

    /// Write a current-schema single-pane workspace referencing `pane_id`.
    fn persist_workspace(state_dir: &Path, runtime_id: Uuid, pane_id: PaneId) {
        let mut tree = WorkspaceTree::new();
        tree.insert_root(pane_id);
        let workspace = WorkspaceFileV2 {
            schema_version: RUNTIME_FILE_SCHEMA_VERSION,
            spec: WorkspaceSpecV2 {
                id: runtime_id,
                name: "ws".into(),
                policy: WorkspacePolicy::Persistent,
                created_at: SystemTime::now(),
                tree,
                panes: vec![PaneSpecV2 {
                    id: pane_id,
                    cwd: None,
                    title: None,
                    exit_status: None,
                    cols: 80,
                    rows: 24,
                    no_persist: false,
                }],
            },
            instance: WorkspaceInstanceV1 {
                revision: 1,
                last_active_at: SystemTime::now(),
                last_snapshot_at: SystemTime::now(),
            },
        };
        save_daemon_index(state_dir, &[runtime_id]).unwrap();
        save_workspace(state_dir, &workspace).unwrap();
    }

    fn write_hist(state_dir: &Path, runtime_id: Uuid, pane_id: Uuid, contents: &str) -> PathBuf {
        let path = layout::history_file(state_dir, runtime_id, pane_id);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn referenced_histfile_is_not_orphaned() {
        let tmp = TempDir::new().unwrap();
        let state = tmp.path();
        let rt = Uuid::new_v4();
        let live = PaneId::new();
        persist_workspace(state, rt, live);
        write_hist(state, rt, live.uuid(), "echo live\n");

        assert!(scan_orphans(state).is_empty(), "a referenced pane's history is not an orphan");
    }

    #[test]
    fn unreferenced_histfile_in_live_workspace_is_orphaned() {
        let tmp = TempDir::new().unwrap();
        let state = tmp.path();
        let rt = Uuid::new_v4();
        let live = PaneId::new();
        persist_workspace(state, rt, live);
        write_hist(state, rt, live.uuid(), "echo live\n");
        // A leftover history file from an old random pane id under the same dir.
        let stale = Uuid::new_v4();
        write_hist(state, rt, stale, "echo recover me\n");

        let orphans = scan_orphans(state);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].pane_id, stale);
        assert_eq!(orphans[0].runtime_id, rt);
    }

    #[test]
    fn workspace_without_panes_orphans_all_history() {
        let tmp = TempDir::new().unwrap();
        let state = tmp.path();
        let rt = Uuid::new_v4();
        // A v1 (unsupported older-version) workspace.json: clean-break, references no panes.
        let path = layout::runtime_file(state, rt);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"schema_version":1,"spec":{},"instance":{}}"#).unwrap();
        let p1 = Uuid::new_v4();
        let p2 = Uuid::new_v4();
        write_hist(state, rt, p1, "history one\n");
        write_hist(state, rt, p2, "history two\n");

        let mut ids: Vec<_> = scan_orphans(state).into_iter().map(|o| o.pane_id).collect();
        ids.sort();
        let mut expected = vec![p1, p2];
        expected.sort();
        assert_eq!(
            ids, expected,
            "every history file under a workspace that references no panes is orphaned"
        );
    }

    #[test]
    fn empty_histfiles_are_skipped() {
        let tmp = TempDir::new().unwrap();
        let state = tmp.path();
        let rt = Uuid::new_v4();
        persist_workspace(state, rt, PaneId::new());
        write_hist(state, rt, Uuid::new_v4(), "");

        assert!(scan_orphans(state).is_empty(), "empty history files carry nothing to recover");
    }

    #[test]
    fn non_hist_and_non_uuid_files_are_ignored() {
        let tmp = TempDir::new().unwrap();
        let state = tmp.path();
        let rt = Uuid::new_v4();
        persist_workspace(state, rt, PaneId::new());
        let hist_dir = layout::runtime_dir(state, rt).join("history");
        std::fs::create_dir_all(&hist_dir).unwrap();
        std::fs::write(hist_dir.join("notes.txt"), "not history\n").unwrap();
        std::fs::write(hist_dir.join("not-a-uuid.hist"), "garbage\n").unwrap();

        assert!(scan_orphans(state).is_empty());
    }

    #[test]
    fn scan_returns_empty_when_state_dir_missing() {
        let tmp = TempDir::new().unwrap();
        assert!(scan_orphans(&tmp.path().join("nope")).is_empty());
    }

    #[test]
    fn export_copies_orphans_into_recovery_dir_preserving_provenance() {
        let tmp = TempDir::new().unwrap();
        let state = tmp.path().join("state");
        let rt = Uuid::new_v4();
        persist_workspace(&state, rt, PaneId::new());
        let stale = Uuid::new_v4();
        write_hist(&state, rt, stale, "echo recover me\n");

        let orphans = scan_orphans(&state);
        assert_eq!(orphans.len(), 1);

        let dest = tmp.path().join("recovery");
        let report = export_orphans(&orphans, &dest).unwrap();

        let copied = dest.join(rt.to_string()).join(format!("{stale}.hist"));
        assert!(copied.exists(), "orphan must be copied under <dest>/<workspace>/<pane>.hist");
        assert_eq!(std::fs::read_to_string(&copied).unwrap(), "echo recover me\n");
        assert_eq!(report.exported.len(), 1);
        assert_eq!(report.total_bytes, "echo recover me\n".len() as u64);
        assert_eq!(report.dest, dest);
    }

    #[test]
    fn export_never_touches_the_source_runtime_directory() {
        let tmp = TempDir::new().unwrap();
        let state = tmp.path().join("state");
        let rt = Uuid::new_v4();
        let live = PaneId::new();
        persist_workspace(&state, rt, live);
        let live_hist = write_hist(&state, rt, live.uuid(), "echo live\n");
        let stale = Uuid::new_v4();
        let stale_hist = write_hist(&state, rt, stale, "echo stale\n");

        let orphans = scan_orphans(&state);
        export_orphans(&orphans, &tmp.path().join("recovery")).unwrap();

        // Both the live and orphaned source files are left exactly as they were.
        assert!(live_hist.exists());
        assert!(stale_hist.exists());
        assert_eq!(std::fs::read_to_string(&stale_hist).unwrap(), "echo stale\n");
    }
}
