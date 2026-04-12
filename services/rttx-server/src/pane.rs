//! Pane management.
//!
//! Each pane represents a single terminal within a session, backed by a PTY
//! process and an in-memory screen state.

use crate::screen::PaneScreen;
use crate::serialization::scrollback_log_path;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use uuid::Uuid;

/// Default scrollback byte limit per pane (10 MB).
const DEFAULT_MAX_SCROLLBACK: usize = 10 * 1024 * 1024;

/// Default on-disk scrollback log byte limit per pane (10 MB).
const DEFAULT_MAX_SCROLLBACK_LOG: u64 = 10 * 1024 * 1024;

/// Maximum bytes sent in a snapshot to a reconnecting client (256 KB).
///
/// The full in-memory buffer can be up to 10 MB, but replaying all of it
/// into VTE on the client is slow and the leading bytes may start
/// mid-escape-sequence after the scrollback cap truncation. This smaller
/// cap keeps reconnect fast and avoids formatting corruption.
pub const MAX_SNAPSHOT_BYTES: usize = 256 * 1024;

/// What changed after feeding PTY output to a pane.
pub struct FeedResult {
    /// New CWD if it changed from the previous value.
    pub new_cwd: Option<String>,
    /// New title if it changed from the previous value.
    pub new_title: Option<String>,
    /// Pending replies to write back to the PTY (e.g. CPR for DSR).
    pub pending_replies: Vec<Vec<u8>>,
}

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
    /// Whether this pane was resurrected from persisted state.
    pub reconstructed: bool,
    /// Path to the scrollback log file.
    pub scrollback_log_path: Option<PathBuf>,
    /// Bytes received since last flush to disk.
    pending_flush: Vec<u8>,
    /// PID of the child process, used to read CWD from /proc.
    pub child_pid: Option<u32>,
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
            reconstructed: false,
            scrollback_log_path: None,
            pending_flush: Vec::new(),
            child_pid: None,
        }
    }

    /// Feed PTY output into the screen state.
    pub fn feed_output(&mut self, data: &[u8]) -> FeedResult {
        self.screen.feed(data);
        self.pending_flush.extend_from_slice(data);
        let new_title = self.screen.title().and_then(|title| {
            let title = title.to_string();
            if self.title.as_deref() == Some(&title) {
                None
            } else {
                self.title = Some(title.clone());
                Some(title)
            }
        });
        let new_cwd = self.screen.cwd().and_then(|cwd| {
            let cwd = cwd.to_string();
            if self.cwd.as_deref() == Some(&cwd) {
                None
            } else {
                self.cwd = Some(cwd.clone());
                Some(cwd)
            }
        });
        let pending_replies = self.screen.take_pending_replies();
        FeedResult { new_cwd, new_title, pending_replies }
    }

    /// Flush pending scrollback bytes to the log file on disk.
    ///
    /// Appends only the bytes received since the last flush. If the file
    /// exceeds `DEFAULT_MAX_SCROLLBACK_LOG` bytes after appending, the file
    /// is truncated to keep only the tail.
    pub fn flush_scrollback(
        &mut self,
        cache_dir: &Path,
        session_id: Uuid,
    ) -> Result<(), std::io::Error> {
        if self.pending_flush.is_empty() {
            return Ok(());
        }

        let path = scrollback_log_path(cache_dir, session_id, self.id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        file.write_all(&self.pending_flush)?;
        self.pending_flush.clear();
        self.scrollback_log_path = Some(path.clone());

        // Cap the file size.
        let meta = std::fs::metadata(&path)?;
        if meta.len() > DEFAULT_MAX_SCROLLBACK_LOG {
            truncate_log_tail(&path, DEFAULT_MAX_SCROLLBACK_LOG)?;
        }

        Ok(())
    }

    /// Whether there are unflushed scrollback bytes.
    #[must_use]
    pub const fn has_pending_flush(&self) -> bool {
        !self.pending_flush.is_empty()
    }

    /// Read the child process CWD from /proc/<pid>/cwd.
    /// Returns None if the PID is unknown or the read fails.
    #[must_use]
    pub fn read_proc_cwd(&self) -> Option<String> {
        let pid = self.child_pid?;
        std::fs::read_link(format!("/proc/{pid}/cwd"))
            .ok()
            .and_then(|p| p.to_str().map(str::to_string))
    }

    /// Return the effective CWD: OSC 7 value if available, otherwise /proc fallback.
    #[must_use]
    pub fn effective_cwd(&self) -> Option<String> {
        self.cwd.clone().or_else(|| self.read_proc_cwd())
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
        let cwd = self.cwd.clone().or_else(|| self.read_proc_cwd());
        PersistedPane {
            id: self.id,
            cwd,
            title: self.title.clone(),
            scrollback_log_path: self.scrollback_log_path.clone().unwrap_or_default(),
            exit_status: self.exit_status,
            cols: self.cols,
            rows: self.rows,
        }
    }
}

/// Truncate a log file to keep only the last `max_bytes` bytes.
///
/// Reads the tail, rewrites the file. This is `O(max_bytes)` but runs at most
/// once per flush cycle and only when the file exceeds the cap.
fn truncate_log_tail(path: &Path, max_bytes: u64) -> Result<(), std::io::Error> {
    let data = std::fs::read(path)?;
    let keep_from = data.len().saturating_sub(max_bytes as usize);
    std::fs::write(path, &data[keep_from..])?;
    Ok(())
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
        assert!(!pane.reconstructed);
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
    fn feed_output_updates_current_directory_from_osc7() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"\x1b]7;file://localhost/tmp/project\x07");
        assert_eq!(pane.cwd.as_deref(), Some("/tmp/project"));
    }

    #[test]
    fn flush_scrollback_creates_log_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_id = Uuid::new_v4();
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"hello world");
        assert!(pane.has_pending_flush());

        pane.flush_scrollback(tmp.path(), session_id).unwrap();
        assert!(!pane.has_pending_flush());

        let log_path = pane.scrollback_log_path.as_ref().unwrap();
        assert!(log_path.exists());
        let content = std::fs::read(log_path).unwrap();
        assert_eq!(content, b"hello world");
    }

    #[test]
    fn flush_scrollback_appends_incrementally() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_id = Uuid::new_v4();
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);

        pane.feed_output(b"first ");
        pane.flush_scrollback(tmp.path(), session_id).unwrap();

        pane.feed_output(b"second");
        pane.flush_scrollback(tmp.path(), session_id).unwrap();

        let log_path = pane.scrollback_log_path.as_ref().unwrap();
        let content = std::fs::read(log_path).unwrap();
        assert_eq!(content, b"first second");
    }

    #[test]
    fn flush_scrollback_noop_when_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_id = Uuid::new_v4();
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);

        assert!(!pane.has_pending_flush());
        pane.flush_scrollback(tmp.path(), session_id).unwrap();
        assert!(pane.scrollback_log_path.is_none());
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

    #[test]
    fn effective_cwd_prefers_osc7_over_proc() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.cwd = Some("/osc7/path".into());
        pane.child_pid = Some(1); // won't be read
        assert_eq!(pane.effective_cwd().as_deref(), Some("/osc7/path"));
    }

    #[test]
    fn effective_cwd_returns_none_without_pid_or_osc7() {
        let pane = Pane::new(Uuid::new_v4(), 80, 24);
        assert!(pane.effective_cwd().is_none());
    }

    #[test]
    fn flush_scrollback_caps_file_size() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_id = Uuid::new_v4();
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);

        // Write more than DEFAULT_MAX_SCROLLBACK_LOG in multiple flushes.
        let chunk = vec![b'A'; 4 * 1024 * 1024]; // 4 MB
        for _ in 0..4 {
            pane.feed_output(&chunk);
            pane.flush_scrollback(tmp.path(), session_id).unwrap();
        }
        // 4 * 4 MB = 16 MB written, cap is 10 MB.
        let log_path = pane.scrollback_log_path.as_ref().unwrap();
        let size = std::fs::metadata(log_path).unwrap().len();
        assert!(
            size <= DEFAULT_MAX_SCROLLBACK_LOG,
            "scrollback log {size} bytes exceeds cap {DEFAULT_MAX_SCROLLBACK_LOG}"
        );
        assert!(size > 0, "scrollback log should not be empty");
    }

    #[test]
    fn truncate_log_tail_keeps_end() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("test.log");
        std::fs::write(&path, b"AAABBBCCC").unwrap();
        truncate_log_tail(&path, 3).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"CCC");
    }

    #[test]
    fn feed_output_returns_new_cwd_on_osc7() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        assert!(pane.cwd.is_none());

        // OSC 7 with file URI.
        let osc7 = b"\x1b]7;file://localhost/tmp/test\x07";
        let result = pane.feed_output(osc7);
        assert_eq!(result.new_cwd.as_deref(), Some("/tmp/test"));
        assert_eq!(pane.cwd.as_deref(), Some("/tmp/test"));
    }

    #[test]
    fn feed_output_returns_none_when_cwd_unchanged() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);

        let osc7 = b"\x1b]7;file://localhost/tmp/test\x07";
        let result = pane.feed_output(osc7);
        assert!(result.new_cwd.is_some());

        // Same CWD again — should not report a change.
        let result = pane.feed_output(osc7);
        assert!(result.new_cwd.is_none());
    }

    #[test]
    fn feed_output_returns_none_for_plain_output() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        let result = pane.feed_output(b"hello world\r\n");
        assert!(result.new_cwd.is_none());
        assert!(result.new_title.is_none());
    }

    #[test]
    fn feed_output_returns_new_title_on_osc0() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        let osc0 = b"\x1b]0;my-project\x07";
        let result = pane.feed_output(osc0);
        assert_eq!(result.new_title.as_deref(), Some("my-project"));
        assert_eq!(pane.title.as_deref(), Some("my-project"));
    }

    #[test]
    fn feed_output_returns_none_when_title_unchanged() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        let osc0 = b"\x1b]0;my-project\x07";
        let result = pane.feed_output(osc0);
        assert!(result.new_title.is_some());

        let result = pane.feed_output(osc0);
        assert!(result.new_title.is_none());
    }

    #[test]
    fn feed_output_returns_dsr_reply() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"abc");
        let result = pane.feed_output(b"\x1b[6n");
        assert_eq!(result.pending_replies.len(), 1);
        assert_eq!(result.pending_replies[0], b"\x1b[1;4R");
    }

    #[test]
    fn feed_output_returns_empty_replies_for_plain_output() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        let result = pane.feed_output(b"hello");
        assert!(result.pending_replies.is_empty());
    }
}
