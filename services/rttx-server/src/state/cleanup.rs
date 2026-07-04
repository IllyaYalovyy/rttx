//! Workspace and pane directory cleanup (RFC-022 §7, RFC-031 §8).
//!
//! Cleanup is explicit and close-driven, keyed on pane-tree membership:
//!
//! - On workspace delete: remove `workspaces/<id>/` in a background task.
//! - On pane close: remove that pane's durable artifacts (screen snapshot,
//!   scrollback log, history file, generated shell-init dir) in a background
//!   task. A pane that leaves the tree leaves nothing behind.
//!
//! Cleanup is driven entirely by explicit pane and workspace lifecycle events.
//! `PaneId`s (RFC-031) nothing is ever left unreferenced, so a sweep would only
//! mask bugs rather than fix them.

use crate::state::layout;
use std::path::Path;
use uuid::Uuid;

/// Remove a workspace's directory in a background task.
///
/// Errors are logged but do not propagate — the caller should not block
/// on cleanup of a terminated workspace.
pub fn remove_runtime_dir_background(state_dir: &Path, runtime_id: Uuid) {
    let dir = layout::runtime_dir(state_dir, runtime_id);
    let short = &runtime_id.to_string()[..8];
    if !dir.exists() {
        return;
    }
    let short = short.to_string();
    std::thread::spawn(move || {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            tracing::error!("Failed to remove workspace directory for {short}: {e}");
        } else {
            tracing::info!("Removed workspace directory for {short}");
        }
    });
}

/// Remove a single pane's durable artifacts in a background task.
///
/// Called when a pane is closed and thus leaves the workspace tree. The caller
/// holds the server lock, so the file I/O is deferred to a background thread.
pub fn remove_pane_state_background(state_dir: &Path, runtime_id: Uuid, pane_id: Uuid) {
    let state_dir = state_dir.to_path_buf();
    std::thread::spawn(move || {
        remove_pane_state(&state_dir, runtime_id, pane_id);
    });
}

/// Remove every durable artifact keyed on `pane_id`: screen snapshot,
/// scrollback log, history file, and the generated shell-init directory.
///
/// Missing entries are not an error. Failures are logged but never propagate.
pub fn remove_pane_state(state_dir: &Path, runtime_id: Uuid, pane_id: Uuid) {
    let short = &pane_id.to_string()[..8];
    remove_file_if_present(&layout::screen_snapshot(state_dir, runtime_id, pane_id), short);
    remove_file_if_present(&layout::scrollback_log(state_dir, runtime_id, pane_id), short);
    remove_file_if_present(&layout::history_file(state_dir, runtime_id, pane_id), short);

    let shell_init = layout::shell_init_dir(state_dir, runtime_id, pane_id);
    if shell_init.exists()
        && let Err(e) = std::fs::remove_dir_all(&shell_init)
    {
        tracing::error!("Failed to remove shell-init dir for pane {short}: {e}");
    }
    tracing::info!("Removed durable state for closed pane {short}");
}

/// Remove a file if it exists, logging any failure.
fn remove_file_if_present(path: &Path, short_pane: &str) {
    if path.exists()
        && let Err(e) = std::fs::remove_file(path)
    {
        tracing::error!("Failed to remove {} for pane {short_pane}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::layout;
    use crate::state::persistence;
    use crate::state::types::*;
    use crate::workspace::WorkspacePolicy;
    use std::time::{Duration, SystemTime};
    use tempfile::TempDir;
    use uuid::Uuid;

    fn sample_runtime_file(id: Uuid) -> WorkspaceFileV2 {
        WorkspaceFileV2 {
            schema_version: RUNTIME_FILE_SCHEMA_VERSION,
            spec: WorkspaceSpecV2 {
                id,
                name: "test".into(),
                policy: WorkspacePolicy::Persistent,
                created_at: SystemTime::now(),
                tree: crate::pane_tree::WorkspaceTree::new(),
                panes: vec![],
            },
            instance: WorkspaceInstanceV1 {
                revision: 1,
                last_active_at: SystemTime::now(),
                last_snapshot_at: SystemTime::now(),
            },
        }
    }

    /// Create the four durable artifacts for a pane on disk.
    fn seed_pane_state(state_dir: &Path, runtime_id: Uuid, pane_id: Uuid) {
        let screen = layout::screen_snapshot(state_dir, runtime_id, pane_id);
        let scroll = layout::scrollback_log(state_dir, runtime_id, pane_id);
        let hist = layout::history_file(state_dir, runtime_id, pane_id);
        let shell_init = layout::shell_init_dir(state_dir, runtime_id, pane_id);
        for f in [&screen, &scroll, &hist] {
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, b"x").unwrap();
        }
        std::fs::create_dir_all(&shell_init).unwrap();
        std::fs::write(shell_init.join("rcfile"), b"x").unwrap();
    }

    fn pane_state_exists(state_dir: &Path, runtime_id: Uuid, pane_id: Uuid) -> bool {
        layout::screen_snapshot(state_dir, runtime_id, pane_id).exists()
            || layout::scrollback_log(state_dir, runtime_id, pane_id).exists()
            || layout::history_file(state_dir, runtime_id, pane_id).exists()
            || layout::shell_init_dir(state_dir, runtime_id, pane_id).exists()
    }

    #[test]
    fn remove_runtime_dir_background_removes_directory() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let rt_id = Uuid::new_v4();

        // Create a workspace directory with some content.
        let rf = sample_runtime_file(rt_id);
        persistence::save_workspace(state_dir, &rf).unwrap();
        let dir = layout::runtime_dir(state_dir, rt_id);
        assert!(dir.exists());

        remove_runtime_dir_background(state_dir, rt_id);

        // Wait for background thread to finish.
        std::thread::sleep(Duration::from_millis(200));
        assert!(!dir.exists());
    }

    #[test]
    fn remove_runtime_dir_background_noop_when_missing() {
        let tmp = TempDir::new().unwrap();
        // Should not panic or error.
        remove_runtime_dir_background(tmp.path(), Uuid::new_v4());
    }

    #[test]
    fn remove_pane_state_removes_all_durable_artifacts() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let runtime_id = Uuid::new_v4();
        let pane_id = Uuid::new_v4();

        seed_pane_state(state_dir, runtime_id, pane_id);
        assert!(pane_state_exists(state_dir, runtime_id, pane_id));

        remove_pane_state(state_dir, runtime_id, pane_id);

        assert!(
            !pane_state_exists(state_dir, runtime_id, pane_id),
            "screen, scrollback, history, and shell-init must all be removed"
        );
    }

    #[test]
    fn remove_pane_state_leaves_sibling_panes_untouched() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let runtime_id = Uuid::new_v4();
        let closed = Uuid::new_v4();
        let kept = Uuid::new_v4();

        seed_pane_state(state_dir, runtime_id, closed);
        seed_pane_state(state_dir, runtime_id, kept);

        remove_pane_state(state_dir, runtime_id, closed);

        assert!(!pane_state_exists(state_dir, runtime_id, closed));
        assert!(
            pane_state_exists(state_dir, runtime_id, kept),
            "closing one pane must not remove a sibling pane's state"
        );
    }

    #[test]
    fn remove_pane_state_noop_when_missing() {
        let tmp = TempDir::new().unwrap();
        // No artifacts on disk: must not panic.
        remove_pane_state(tmp.path(), Uuid::new_v4(), Uuid::new_v4());
    }

    #[test]
    fn remove_pane_state_background_removes_artifacts() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let runtime_id = Uuid::new_v4();
        let pane_id = Uuid::new_v4();

        seed_pane_state(state_dir, runtime_id, pane_id);
        remove_pane_state_background(state_dir, runtime_id, pane_id);

        std::thread::sleep(Duration::from_millis(200));
        assert!(!pane_state_exists(state_dir, runtime_id, pane_id));
    }
}
