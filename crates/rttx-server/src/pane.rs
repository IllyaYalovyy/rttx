//! Pane management.
//!
//! Each pane represents a single terminal within a session, backed by a PTY
//! process and an in-memory screen state.

use crate::screen::PaneScreen;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;
use uuid::Uuid;

/// Default scrollback byte limit per pane (10 MB).
const DEFAULT_MAX_SCROLLBACK: usize = 10 * 1024 * 1024;

/// Runtime state of a single pane.
pub struct Pane {
    /// Unique pane identifier.
    pub id: Uuid,
    /// In-memory screen state.
    pub screen: PaneScreen,
    /// Current working directory (updated via OSC 7 or similar).
    pub cwd: Option<String>,
    /// Pane title (set by user or OSC escape).
    pub title: Option<String>,
    /// Terminal size.
    pub cols: u16,
    /// Terminal size.
    pub rows: u16,
    /// Exit status if the pane process has exited.
    pub exit_status: Option<i32>,
    /// Path to the scrollback log file.
    pub scrollback_log_path: Option<PathBuf>,
}

impl Pane {
    /// Create a new pane with default screen state.
    #[must_use]
    pub fn new(id: Uuid, cols: u16, rows: u16) -> Self {
        Self {
            id,
            screen: PaneScreen::new(DEFAULT_MAX_SCROLLBACK),
            cwd: None,
            title: None,
            cols,
            rows,
            exit_status: None,
            scrollback_log_path: None,
        }
    }

    /// Feed PTY output into the screen state.
    pub fn feed_output(&mut self, data: &[u8]) {
        self.screen.feed(data);
        if let Some(title) = self.screen.title() {
            self.title = Some(title.to_string());
        }
    }

    /// Mark the pane as exited.
    pub const fn set_exited(&mut self, status: i32) {
        self.exit_status = Some(status);
    }

    /// Whether the pane process has exited.
    #[must_use]
    pub const fn is_exited(&self) -> bool {
        self.exit_status.is_some()
    }

    /// Build a persistable snapshot of this pane.
    #[must_use]
    pub fn to_persisted(&self) -> PersistedPane {
        PersistedPane {
            id: self.id,
            cwd: self.cwd.clone(),
            title: self.title.clone(),
            scrollback_log_path: self.scrollback_log_path.clone().unwrap_or_default(),
            exit_status: self.exit_status,
            cols: self.cols,
            rows: self.rows,
        }
    }
}

/// Serializable pane state for disk persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedPane {
    /// Unique pane identifier.
    pub id: Uuid,
    /// Last known working directory.
    pub cwd: Option<String>,
    /// Pane title.
    pub title: Option<String>,
    /// Path to the scrollback log file.
    pub scrollback_log_path: PathBuf,
    /// Exit status if the process exited.
    pub exit_status: Option<i32>,
    /// Terminal columns.
    pub cols: u16,
    /// Terminal rows.
    pub rows: u16,
}

/// History entry for per-session command history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// The command text.
    pub command: String,
    /// Working directory when the command was run.
    pub cwd: String,
    /// When the command was executed.
    pub timestamp: SystemTime,
    /// Which pane the command was run in.
    pub pane_id: Uuid,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_pane_is_not_exited() {
        let pane = Pane::new(Uuid::new_v4(), 80, 24);
        assert!(!pane.is_exited());
    }

    #[test]
    fn pane_exit_status() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.set_exited(0);
        assert!(pane.is_exited());
        assert_eq!(pane.exit_status, Some(0));
    }

    #[test]
    fn feed_output_updates_screen() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"hello");
        assert_eq!(pane.screen.raw_bytes(), b"hello");
    }

    #[test]
    fn persisted_roundtrip() {
        let pane = Pane::new(Uuid::new_v4(), 80, 24);
        let persisted = pane.to_persisted();
        let json = serde_json::to_string(&persisted).unwrap();
        let recovered: PersistedPane = serde_json::from_str(&json).unwrap();
        assert_eq!(recovered.id, pane.id);
        assert_eq!(recovered.cols, 80);
    }
}
