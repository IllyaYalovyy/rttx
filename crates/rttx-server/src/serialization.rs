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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::PersistedPane;
    use crate::session::PersistedSession;
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
}
