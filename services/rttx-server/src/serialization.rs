//! Periodic state serialization to disk.
//!
//! Writes the full server state atomically (write-to-tmp then rename) every
//! tick. On startup, loads persisted state to resurrect sessions.

use crate::session::PersistedSession;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Top-level persisted server state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerState {
    /// All persisted sessions.
    pub sessions: Vec<PersistedSession>,
    /// When this snapshot was taken.
    pub serialized_at: SystemTime,
    /// Server version that wrote this state.
    pub server_version: String,
}

impl ServerState {
    /// Create an empty state snapshot.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            sessions: Vec::new(),
            serialized_at: SystemTime::now(),
            server_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// Write state to disk atomically.
///
/// Writes to a `.tmp` file first, then renames to the final path. This
/// prevents corrupt state files if the process crashes mid-write.
pub fn write_state_atomic(state: &ServerState, path: &Path) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("tmp");
    let json = serde_json::to_string_pretty(state).map_err(std::io::Error::other)?;
    std::fs::write(&tmp_path, json)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Load persisted state from disk.
///
/// Returns `None` if the file does not exist. Returns an error on I/O or
/// parse failures.
pub fn load_state(path: &Path) -> Result<Option<ServerState>, std::io::Error> {
    if !path.exists() {
        return Ok(None);
    }
    let json = std::fs::read_to_string(path)?;
    let state: ServerState = serde_json::from_str(&json)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(Some(state))
}

/// Return the default state file path.
#[must_use]
pub fn default_state_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("state.json")
}

/// Return the scrollback log directory for a session/pane.
#[must_use]
pub fn scrollback_log_path(
    cache_dir: &Path,
    session_id: uuid::Uuid,
    pane_id: uuid::Uuid,
) -> PathBuf {
    cache_dir.join("scrollback").join(session_id.to_string()).join(format!("{pane_id}.log"))
}

/// Return the shell history file path for a pane.
#[must_use]
pub fn history_path(cache_dir: &Path, session_id: uuid::Uuid, pane_id: uuid::Uuid) -> PathBuf {
    cache_dir.join("history").join(session_id.to_string()).join(format!("{pane_id}.hist"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::PersistedPane;
    use crate::session::{PersistedSession, RuntimePolicy};
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;
    use uuid::Uuid;

    fn sample_state() -> ServerState {
        ServerState {
            sessions: vec![PersistedSession {
                id: Uuid::new_v4(),
                name: "test-session".into(),
                panes: vec![PersistedPane {
                    id: Uuid::new_v4(),
                    cwd: Some("/home/user".into()),
                    title: Some("bash".into()),
                    scrollback_log_path: PathBuf::from("/tmp/scrollback.log"),
                    exit_status: None,
                    cols: 80,
                    rows: 24,
                }],
                active_pane_id: None,
                command_history: Vec::new(),
                policy: RuntimePolicy::Persistent,
                revision: 3,
                created_at: SystemTime::now(),
                last_active_at: SystemTime::now(),
            }],
            serialized_at: SystemTime::now(),
            server_version: "0.1.0".into(),
        }
    }

    #[test]
    fn write_and_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("state.json");
        let state = sample_state();

        write_state_atomic(&state, &path).unwrap();
        let loaded = load_state(&path).unwrap().unwrap();

        assert_eq!(loaded.sessions.len(), 1);
        assert_eq!(loaded.sessions[0].name, "test-session");
        assert_eq!(loaded.sessions[0].panes.len(), 1);
        assert_eq!(loaded.sessions[0].panes[0].cols, 80);
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent.json");
        let result = load_state(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn atomic_write_creates_parent_dirs() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("deep").join("nested").join("state.json");
        let state = ServerState::empty();
        write_state_atomic(&state, &path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn empty_state_serializes() {
        let state = ServerState::empty();
        let json = serde_json::to_string(&state).unwrap();
        let recovered: ServerState = serde_json::from_str(&json).unwrap();
        assert!(recovered.sessions.is_empty());
    }

    #[test]
    fn scrollback_log_path_format() {
        let cache = PathBuf::from("/tmp/cache");
        let sid = Uuid::new_v4();
        let pid = Uuid::new_v4();
        let path = scrollback_log_path(&cache, sid, pid);
        assert!(path.starts_with("/tmp/cache/scrollback"));
        assert!(path.to_string_lossy().ends_with(".log"));
    }

    #[test]
    fn load_corrupt_json_returns_error() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("state.json");
        std::fs::write(&path, "{ this is not valid json }").unwrap();
        let result = load_state(&path);
        assert!(result.is_err());
    }

    #[test]
    fn load_truncated_json_returns_error() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("state.json");
        let state = sample_state();
        let json = serde_json::to_string_pretty(&state).unwrap();
        // Write only the first half of the JSON.
        std::fs::write(&path, &json[..json.len() / 2]).unwrap();
        let result = load_state(&path);
        assert!(result.is_err());
    }

    #[test]
    fn load_empty_file_returns_error() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("state.json");
        std::fs::write(&path, "").unwrap();
        let result = load_state(&path);
        assert!(result.is_err());
    }

    #[test]
    fn load_ignores_leftover_tmp_file() {
        let tmp = TempDir::new().unwrap();
        let state_path = tmp.path().join("state.json");
        let tmp_path = state_path.with_extension("tmp");

        // Simulate interrupted write: .tmp exists but .json doesn't.
        std::fs::write(&tmp_path, "partial garbage").unwrap();

        let result = load_state(&state_path).unwrap();
        assert!(result.is_none(), "leftover .tmp must not be loaded as state");
    }

    #[test]
    fn load_state_with_unknown_fields_succeeds() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("state.json");
        // JSON with extra fields that don't exist in the struct.
        let json = r#"{
            "sessions": [],
            "serialized_at": {"secs_since_epoch": 1700000000, "nanos_since_epoch": 0},
            "server_version": "99.0.0",
            "future_field": "should be ignored"
        }"#;
        std::fs::write(&path, json).unwrap();
        let loaded = load_state(&path).unwrap().unwrap();
        assert!(loaded.sessions.is_empty());
    }

    #[test]
    fn write_to_readonly_dir_returns_error() {
        use std::os::unix::fs::PermissionsExt;

        // Root ignores filesystem permission bits, so this test is meaningless as root.
        if std::process::Command::new("id")
            .arg("-u")
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        {
            eprintln!("SKIPPED: running as root");
            return;
        }

        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("readonly");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o444)).unwrap();

        let path = dir.join("state.json");
        let state = ServerState::empty();
        let result = write_state_atomic(&state, &path);
        assert!(result.is_err());

        // Restore permissions for cleanup.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn atomic_write_does_not_leave_tmp_on_success() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("state.json");
        let state = sample_state();
        write_state_atomic(&state, &path).unwrap();

        let tmp_path = path.with_extension("tmp");
        assert!(!tmp_path.exists(), ".tmp file must be cleaned up after successful write");
        assert!(path.exists());
    }

    #[test]
    fn history_path_is_per_session_and_pane() {
        let cache = std::path::Path::new("/cache");
        let s1 = uuid::Uuid::new_v4();
        let p1 = uuid::Uuid::new_v4();
        let p2 = uuid::Uuid::new_v4();
        let h1 = history_path(cache, s1, p1);
        let h2 = history_path(cache, s1, p2);
        assert_ne!(h1, h2, "different panes must have different history files");
        assert!(h1.to_string_lossy().contains(&p1.to_string()));
        assert!(h1.to_string_lossy().ends_with(".hist"));
    }
}
