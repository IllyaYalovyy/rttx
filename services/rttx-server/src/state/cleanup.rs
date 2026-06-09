//! Runtime directory cleanup and orphan sweep (RFC-022 §7).
//!
//! On runtime delete: remove `runtimes/<id>/` in a background task.
//! On startup: move unreferenced runtime directories to `runtimes/.orphans/`.
//! Prune `.orphans/` entries older than 30 days.

use crate::state::layout;
use std::collections::HashSet;
use std::hash::BuildHasher;
use std::path::Path;
use std::time::{Duration, SystemTime};
use uuid::Uuid;

/// How long orphaned directories are kept before pruning.
const ORPHAN_RETENTION: Duration = Duration::from_hours(30 * 24);

/// Remove a runtime's directory in a background task.
///
/// Errors are logged but do not propagate — the caller should not block
/// on cleanup of a terminated runtime.
pub fn remove_runtime_dir_background(state_dir: &Path, runtime_id: Uuid) {
    let dir = layout::runtime_dir(state_dir, runtime_id);
    let short = &runtime_id.to_string()[..8];
    if !dir.exists() {
        return;
    }
    let short = short.to_string();
    std::thread::spawn(move || {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            tracing::error!("Failed to remove runtime directory for {short}: {e}");
        } else {
            tracing::info!("Removed runtime directory for {short}");
        }
    });
}

/// Move unreferenced runtime directories to `.orphans/` and prune old orphans.
///
/// Called once on startup after loading the daemon index. `known_ids` is the
/// set of runtime IDs that the daemon index references (both successfully
/// loaded and failed-to-load runtimes are considered "known").
pub fn sweep_orphans<S: BuildHasher>(state_dir: &Path, known_ids: &HashSet<Uuid, S>) {
    let runtimes = layout::runtimes_dir(state_dir);
    if !runtimes.exists() {
        return;
    }

    let orphans = layout::orphans_dir(state_dir);

    // Phase 1: move unreferenced runtime dirs to .orphans/
    let entries = match std::fs::read_dir(&runtimes) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::error!("Failed to read runtimes directory: {e}");
            return;
        }
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip the .orphans directory itself.
        if name_str == ".orphans" {
            continue;
        }

        // Only consider directories that look like UUIDs.
        let Ok(id) = Uuid::parse_str(&name_str) else {
            continue;
        };

        if known_ids.contains(&id) {
            continue;
        }

        // Move to .orphans/
        if let Err(e) = std::fs::create_dir_all(&orphans) {
            tracing::error!("Failed to create orphans directory: {e}");
            return;
        }
        let dest = orphans.join(&name);
        match std::fs::rename(entry.path(), &dest) {
            Ok(()) => {
                tracing::info!("Moved orphaned runtime {} to .orphans/", &name_str[..8]);
            }
            Err(e) => {
                tracing::error!("Failed to move orphaned runtime {}: {e}", &name_str[..8]);
            }
        }
    }

    // Phase 2: prune old orphans.
    prune_old_orphans(&orphans);
}

/// Remove orphan entries older than [`ORPHAN_RETENTION`].
fn prune_old_orphans(orphans_dir: &Path) {
    if !orphans_dir.exists() {
        return;
    }

    let now = SystemTime::now();
    let entries = match std::fs::read_dir(orphans_dir) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::error!("Failed to read orphans directory: {e}");
            return;
        }
    };

    for entry in entries.flatten() {
        let age = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|mtime| now.duration_since(mtime).ok());

        let Some(age) = age else {
            continue;
        };

        if age > ORPHAN_RETENTION {
            let name = entry.file_name();
            let short = &name.to_string_lossy()[..8.min(name.len())];
            match std::fs::remove_dir_all(entry.path()) {
                Ok(()) => {
                    tracing::info!(
                        "Pruned old orphan {short} (age: {} days)",
                        age.as_secs() / 86400
                    );
                }
                Err(e) => {
                    tracing::error!("Failed to prune orphan {short}: {e}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::RuntimePolicy;
    use crate::state::layout;
    use crate::state::persistence;
    use crate::state::types::*;
    use std::collections::HashSet;
    use tempfile::TempDir;

    fn sample_runtime_file(id: Uuid) -> WorkspaceFileV2 {
        WorkspaceFileV2 {
            schema_version: RUNTIME_FILE_SCHEMA_VERSION,
            spec: WorkspaceSpecV2 {
                id,
                name: "test".into(),
                policy: RuntimePolicy::Persistent,
                created_at: SystemTime::now(),
                tree: crate::pane_tree::WorkspaceTree::new(),
                panes: vec![],
            },
            instance: RuntimeInstanceV1 {
                revision: 1,
                last_active_at: SystemTime::now(),
                last_snapshot_at: SystemTime::now(),
            },
        }
    }

    #[test]
    fn remove_runtime_dir_background_removes_directory() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();
        let rt_id = Uuid::new_v4();

        // Create a runtime directory with some content.
        let rf = sample_runtime_file(rt_id);
        persistence::save_runtime(state_dir, &rf).unwrap();
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
    fn sweep_orphans_moves_unreferenced_dirs() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();

        let known_id = Uuid::new_v4();
        let orphan_id = Uuid::new_v4();

        // Create directories for both.
        persistence::save_runtime(state_dir, &sample_runtime_file(known_id)).unwrap();
        persistence::save_runtime(state_dir, &sample_runtime_file(orphan_id)).unwrap();

        let known_ids: HashSet<Uuid> = std::iter::once(known_id).collect();
        sweep_orphans(state_dir, &known_ids);

        // Known runtime dir still exists.
        assert!(layout::runtime_dir(state_dir, known_id).exists());

        // Orphan was moved.
        assert!(!layout::runtime_dir(state_dir, orphan_id).exists());
        let orphan_dest = layout::orphans_dir(state_dir).join(orphan_id.to_string());
        assert!(orphan_dest.exists());
    }

    #[test]
    fn sweep_orphans_skips_orphans_dir_itself() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();

        // Create .orphans/ with some content.
        let orphans = layout::orphans_dir(state_dir);
        std::fs::create_dir_all(&orphans).unwrap();
        std::fs::write(orphans.join("marker"), "test").unwrap();

        let known_ids: HashSet<Uuid> = HashSet::new();
        sweep_orphans(state_dir, &known_ids);

        // .orphans/ should still exist.
        assert!(orphans.exists());
    }

    #[test]
    fn sweep_orphans_skips_non_uuid_directories() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();

        let runtimes = layout::runtimes_dir(state_dir);
        std::fs::create_dir_all(&runtimes).unwrap();
        let non_uuid = runtimes.join("not-a-uuid");
        std::fs::create_dir_all(&non_uuid).unwrap();

        let known_ids: HashSet<Uuid> = HashSet::new();
        sweep_orphans(state_dir, &known_ids);

        // Non-UUID directory should be left alone.
        assert!(non_uuid.exists());
    }

    #[test]
    fn sweep_orphans_prunes_old_entries() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();

        let orphans = layout::orphans_dir(state_dir);
        std::fs::create_dir_all(&orphans).unwrap();

        // Create an old orphan (set mtime to 31 days ago).
        let old_id = Uuid::new_v4();
        let old_dir = orphans.join(old_id.to_string());
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("runtime.json"), "{}").unwrap();

        let old_time = SystemTime::now() - Duration::from_hours(31 * 24);
        let old_filetime = std::fs::FileTimes::new().set_modified(old_time);
        std::fs::File::open(&old_dir).unwrap().set_times(old_filetime).unwrap();

        // Create a recent orphan.
        let recent_id = Uuid::new_v4();
        let recent_dir = orphans.join(recent_id.to_string());
        std::fs::create_dir_all(&recent_dir).unwrap();

        let known_ids: HashSet<Uuid> = HashSet::new();
        sweep_orphans(state_dir, &known_ids);

        // Old orphan should be pruned.
        assert!(!old_dir.exists());
        // Recent orphan should remain.
        assert!(recent_dir.exists());
    }

    #[test]
    fn sweep_orphans_noop_when_no_runtimes_dir() {
        let tmp = TempDir::new().unwrap();
        let known_ids: HashSet<Uuid> = HashSet::new();
        // Should not panic.
        sweep_orphans(tmp.path(), &known_ids);
    }

    #[test]
    fn sweep_orphans_with_empty_known_ids_moves_all() {
        let tmp = TempDir::new().unwrap();
        let state_dir = tmp.path();

        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        persistence::save_runtime(state_dir, &sample_runtime_file(id1)).unwrap();
        persistence::save_runtime(state_dir, &sample_runtime_file(id2)).unwrap();

        let known_ids: HashSet<Uuid> = HashSet::new();
        sweep_orphans(state_dir, &known_ids);

        assert!(!layout::runtime_dir(state_dir, id1).exists());
        assert!(!layout::runtime_dir(state_dir, id2).exists());

        let orphans = layout::orphans_dir(state_dir);
        assert!(orphans.join(id1.to_string()).exists());
        assert!(orphans.join(id2.to_string()).exists());
    }
}
