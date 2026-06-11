//! V2 per-workspace directory layout path helpers (RFC-022 §1).
//!
//! All paths are relative to the daemon state directory returned by
//! [`OsInterface::state_dir`](crate::os::OsInterface::state_dir).
//!
//! ```text
//! $XDG_STATE_HOME/rttx/daemon/          ← state_dir()
//! ├── daemon.json
//! └── workspaces/
//!     └── <runtime_id>/
//!         ├── workspace.json
//!         ├── screen/<pane_id>.snap
//!         ├── scrollback/<pane_id>.log
//!         └── history/<pane_id>.hist
//! ```

use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Filename for the top-level daemon index.
const DAEMON_INDEX_FILE: &str = "daemon.json";

/// Directory containing per-workspace subdirectories.
const RUNTIMES_DIR: &str = "workspaces";

/// Filename for a workspace's metadata inside its directory.
const RUNTIME_FILE: &str = "workspace.json";

/// Subdirectory for deterministic screen snapshots.
const SCREEN_DIR: &str = "screen";

/// Subdirectory for append-only scrollback logs.
const SCROLLBACK_DIR: &str = "scrollback";

/// Subdirectory for per-pane shell history.
const HISTORY_DIR: &str = "history";

/// Subdirectory for per-pane generated shell-init files (bash rcfile, zsh
/// `ZDOTDIR`).
const SHELL_INIT_DIR: &str = "shell-init";

/// Path to the top-level daemon index file.
#[must_use]
pub fn daemon_index(state_dir: &Path) -> PathBuf {
    state_dir.join(DAEMON_INDEX_FILE)
}

/// Path to the `workspaces/` directory.
#[must_use]
pub fn runtimes_dir(state_dir: &Path) -> PathBuf {
    state_dir.join(RUNTIMES_DIR)
}

/// Path to a specific workspace's directory.
#[must_use]
pub fn runtime_dir(state_dir: &Path, runtime_id: Uuid) -> PathBuf {
    runtimes_dir(state_dir).join(runtime_id.to_string())
}

/// Path to a workspace's metadata file (`workspace.json`).
#[must_use]
pub fn runtime_file(state_dir: &Path, runtime_id: Uuid) -> PathBuf {
    runtime_dir(state_dir, runtime_id).join(RUNTIME_FILE)
}

/// Path to a pane's screen snapshot file.
#[must_use]
pub fn screen_snapshot(state_dir: &Path, runtime_id: Uuid, pane_id: Uuid) -> PathBuf {
    runtime_dir(state_dir, runtime_id).join(SCREEN_DIR).join(format!("{pane_id}.snap"))
}

/// Path to a pane's scrollback log file.
#[must_use]
pub fn scrollback_log(state_dir: &Path, runtime_id: Uuid, pane_id: Uuid) -> PathBuf {
    runtime_dir(state_dir, runtime_id).join(SCROLLBACK_DIR).join(format!("{pane_id}.log"))
}

/// Path to a pane's shell history file.
#[must_use]
pub fn history_file(state_dir: &Path, runtime_id: Uuid, pane_id: Uuid) -> PathBuf {
    runtime_dir(state_dir, runtime_id).join(HISTORY_DIR).join(format!("{pane_id}.hist"))
}

/// Path to a pane's generated shell-init directory (holds the bash rcfile or
/// the zsh `ZDOTDIR` contents). Keyed on `pane_id` so it is stable across
/// shell respawns and daemon restarts.
#[must_use]
pub fn shell_init_dir(state_dir: &Path, runtime_id: Uuid, pane_id: Uuid) -> PathBuf {
    runtime_dir(state_dir, runtime_id).join(SHELL_INIT_DIR).join(pane_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const STATE: &str = "/xdg/state/rttx/daemon";

    fn ids() -> (Uuid, Uuid) {
        let rt = Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();
        let pane = Uuid::parse_str("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").unwrap();
        (rt, pane)
    }

    #[test]
    fn daemon_index_path() {
        let p = daemon_index(Path::new(STATE));
        assert_eq!(p, Path::new("/xdg/state/rttx/daemon/daemon.json"));
    }

    #[test]
    fn runtimes_dir_path() {
        let p = runtimes_dir(Path::new(STATE));
        assert_eq!(p, Path::new("/xdg/state/rttx/daemon/workspaces"));
    }

    #[test]
    fn runtime_dir_path() {
        let (rt, _) = ids();
        let p = runtime_dir(Path::new(STATE), rt);
        assert_eq!(
            p,
            Path::new("/xdg/state/rttx/daemon/workspaces/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")
        );
    }

    #[test]
    fn runtime_file_path() {
        let (rt, _) = ids();
        let p = runtime_file(Path::new(STATE), rt);
        assert!(p.ends_with("workspace.json"));
        assert!(p.starts_with(runtime_dir(Path::new(STATE), rt)));
    }

    #[test]
    fn screen_snapshot_path() {
        let (rt, pane) = ids();
        let p = screen_snapshot(Path::new(STATE), rt, pane);
        assert!(p.to_string_lossy().contains("/screen/"));
        assert!(p.to_string_lossy().ends_with(".snap"));
        assert!(p.starts_with(runtime_dir(Path::new(STATE), rt)));
    }

    #[test]
    fn scrollback_log_path() {
        let (rt, pane) = ids();
        let p = scrollback_log(Path::new(STATE), rt, pane);
        assert!(p.to_string_lossy().contains("/scrollback/"));
        assert!(p.to_string_lossy().ends_with(".log"));
        assert!(p.starts_with(runtime_dir(Path::new(STATE), rt)));
    }

    #[test]
    fn history_file_path() {
        let (rt, pane) = ids();
        let p = history_file(Path::new(STATE), rt, pane);
        assert!(p.to_string_lossy().contains("/history/"));
        assert!(p.to_string_lossy().ends_with(".hist"));
        assert!(p.starts_with(runtime_dir(Path::new(STATE), rt)));
    }

    #[test]
    fn shell_init_dir_path() {
        let (rt, pane) = ids();
        let p = shell_init_dir(Path::new(STATE), rt, pane);
        assert!(p.to_string_lossy().contains("/shell-init/"));
        assert!(p.ends_with(pane.to_string()));
        assert!(p.starts_with(runtime_dir(Path::new(STATE), rt)));
    }

    #[test]
    fn different_panes_produce_different_paths() {
        let rt = Uuid::new_v4();
        let p1 = Uuid::new_v4();
        let p2 = Uuid::new_v4();
        let base = Path::new(STATE);

        assert_ne!(screen_snapshot(base, rt, p1), screen_snapshot(base, rt, p2));
        assert_ne!(scrollback_log(base, rt, p1), scrollback_log(base, rt, p2));
        assert_ne!(history_file(base, rt, p1), history_file(base, rt, p2));
    }

    #[test]
    fn different_workspaces_produce_different_paths() {
        let r1 = Uuid::new_v4();
        let r2 = Uuid::new_v4();
        let pane = Uuid::new_v4();
        let base = Path::new(STATE);

        assert_ne!(runtime_dir(base, r1), runtime_dir(base, r2));
        assert_ne!(runtime_file(base, r1), runtime_file(base, r2));
        assert_ne!(scrollback_log(base, r1, pane), scrollback_log(base, r2, pane));
    }

    #[test]
    fn all_pane_artifacts_share_runtime_dir_prefix() {
        let rt = Uuid::new_v4();
        let pane = Uuid::new_v4();
        let base = Path::new(STATE);
        let rt_dir = runtime_dir(base, rt);

        assert!(screen_snapshot(base, rt, pane).starts_with(&rt_dir));
        assert!(scrollback_log(base, rt, pane).starts_with(&rt_dir));
        assert!(history_file(base, rt, pane).starts_with(&rt_dir));
        assert!(runtime_file(base, rt).starts_with(&rt_dir));
    }
}
