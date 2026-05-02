//! Pane management.
//!
//! Each pane represents a single terminal within a session, backed by a PTY
//! process and an in-memory screen state.

use crate::screen::{PaneScreen, strip_client_queries};
use crate::state::layout::scrollback_log;
use crate::state::types::{SCREEN_SNAPSHOT_SCHEMA_VERSION, ScreenSnapshotV1, TerminalModeSnapshot};
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Default scrollback byte limit per pane (10 MB).
const DEFAULT_MAX_SCROLLBACK: usize = 10 * 1024 * 1024;

/// Default on-disk scrollback log byte limit per pane (10 MB).
const DEFAULT_MAX_SCROLLBACK_LOG: u64 = 10 * 1024 * 1024;

/// Number of rotated scrollback segments to keep (RFC-022 §4).
const SCROLLBACK_ROTATE_KEEP: u32 = 3;

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
    /// Monotonic output sequence counter for v3 `OutputDelta` continuity.
    pub output_seq: u64,
    /// When true, scrollback and history are not flushed to disk (RFC-022 §9).
    pub no_persist: bool,
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
            output_seq: 0,
            no_persist: false,
        }
    }

    /// Feed PTY output into the screen state.
    ///
    /// Combines [`accept_output`] and [`parse_and_extract`] in a single
    /// call. Callers that need to minimize lock hold time should use the
    /// two-phase API instead.
    pub fn feed_output(&mut self, data: &[u8]) -> FeedResult {
        self.accept_output(data);
        self.parse_and_extract(data)
    }

    /// Store raw PTY bytes without running the VTE parser.
    ///
    /// Updates `pending_flush`, `output_seq`, and the screen's raw-byte
    /// buffer. This is a fast memcpy suitable for calling inside a
    /// contended mutex. Call [`parse_and_extract`] afterwards (potentially
    /// after releasing the lock) to run the expensive VTE state machine.
    pub fn accept_output(&mut self, data: &[u8]) {
        self.screen.accept_raw(data);
        self.pending_flush.extend_from_slice(data);
        self.output_seq += 1;
        if self.pending_flush.len() > DEFAULT_MAX_SCROLLBACK {
            let excess = self.pending_flush.len() - DEFAULT_MAX_SCROLLBACK;
            self.pending_flush.drain(..excess);
            self.pending_flush.shrink_to(DEFAULT_MAX_SCROLLBACK);
        }
    }

    /// Temporarily take the screen out of this pane for out-of-lock parsing.
    ///
    /// Returns the screen, replacing it with a default instance. The caller
    /// must call [`return_screen`] to put it back after parsing.
    #[must_use]
    pub fn take_screen(&mut self) -> PaneScreen {
        std::mem::replace(&mut self.screen, PaneScreen::new(DEFAULT_MAX_SCROLLBACK))
    }

    /// Return a previously taken screen and extract metadata changes.
    ///
    /// The screen is placed back into the pane and any CWD, title, or
    /// pending-reply changes are returned.
    pub fn return_screen(&mut self, screen: PaneScreen) -> FeedResult {
        self.screen = screen;
        self.extract_metadata()
    }

    /// Run the VTE parser and extract metadata changes.
    ///
    /// This is the expensive half of [`feed_output`]: it advances the VTE
    /// state machine byte-by-byte and returns any CWD, title, or DSR
    /// reply changes. Callers that split the two phases should call this
    /// after releasing the contended lock.
    pub fn parse_and_extract(&mut self, data: &[u8]) -> FeedResult {
        self.screen.parse(data);
        self.extract_metadata()
    }

    fn extract_metadata(&mut self) -> FeedResult {
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
    ///
    /// No-persist panes skip the disk write entirely; pending bytes are
    /// discarded so the in-memory buffer does not grow unboundedly.
    pub fn flush_scrollback(
        &mut self,
        state_dir: &Path,
        session_id: Uuid,
    ) -> Result<(), std::io::Error> {
        if self.pending_flush.is_empty() {
            return Ok(());
        }

        if self.no_persist {
            self.pending_flush = Vec::new();
            return Ok(());
        }

        let path = scrollback_log(state_dir, session_id, self.id);
        let data = std::mem::take(&mut self.pending_flush);
        write_scrollback_to_disk(&path, &data)?;
        self.scrollback_log_path = Some(path);

        Ok(())
    }

    /// Whether there are unflushed scrollback bytes.
    #[must_use]
    pub const fn has_pending_flush(&self) -> bool {
        !self.pending_flush.is_empty()
    }

    /// Number of bytes waiting to be flushed to disk.
    #[must_use]
    pub const fn pending_flush_len(&self) -> usize {
        self.pending_flush.len()
    }

    /// Drain pending scrollback bytes for out-of-lock flushing.
    ///
    /// Returns the accumulated bytes and resets the internal buffer.
    /// The caller is responsible for writing the returned bytes to disk.
    #[must_use]
    pub fn take_pending_flush(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending_flush)
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

    /// Feed the terminal cleanup sequence into the pane's screen state.
    ///
    /// Called when the pane process exits so that alt-screen, mouse tracking,
    /// hidden cursor, and other TUI modes are reset. Returns the cleanup
    /// bytes for the caller to broadcast to attached clients and append to
    /// the scrollback log.
    pub fn feed_cleanup(&mut self) -> &'static [u8] {
        let cleanup = crate::screen::terminal_cleanup_bytes();
        self.screen.feed(cleanup);
        self.pending_flush.extend_from_slice(cleanup);
        self.output_seq += 1;
        cleanup
    }

    /// Release the in-memory scrollback buffer and pending flush data.
    ///
    /// Called when the pane process exits so the up-to-10 MB `raw_bytes`
    /// buffer does not linger in memory indefinitely.
    pub fn release_scrollback(&mut self) {
        self.screen.clear_scrollback();
        self.pending_flush = Vec::new();
    }

    /// Whether the pane process has exited.
    #[must_use]
    pub const fn is_exited(&self) -> bool {
        self.exit_status.is_some()
    }

    /// Build a deterministic screen snapshot for on-disk persistence (RFC-022 §4).
    #[must_use]
    pub fn to_screen_snapshot(&self) -> ScreenSnapshotV1 {
        let (cursor_row, cursor_col) = self.screen.cursor_position();
        let screen_bytes = self.screen.snapshot_bytes(MAX_SNAPSHOT_BYTES).to_vec();
        ScreenSnapshotV1 {
            schema_version: SCREEN_SNAPSHOT_SCHEMA_VERSION,
            pane_id: self.id,
            cols: self.cols,
            rows: self.rows,
            cursor_row: cursor_row as u16,
            cursor_col: cursor_col as u16,
            cursor_visible: self.screen.cursor_visible(),
            title: self.title.clone(),
            cwd: self.cwd.clone().or_else(|| self.read_proc_cwd()),
            pane_output_seq: self.output_seq,
            modes: TerminalModeSnapshot {
                bracketed_paste: self.screen.bracketed_paste_mode(),
                application_cursor_keys: self.screen.application_cursor_keys(),
                application_keypad: self.screen.application_keypad(),
                mouse_tracking_mode: self.screen.mouse_tracking_mode(),
                sgr_mouse: self.screen.sgr_mouse_mode(),
                focus_reporting: self.screen.focus_event_mode(),
            },
            screen_bytes,
            confidential: self.no_persist,
        }
    }

    /// Restore canonical pane state from a persisted screen snapshot.
    ///
    /// Feeds `screen_bytes` through the parser, then overwrites mode flags,
    /// cursor visibility, and output sequence from the snapshot metadata.
    /// This ensures restart reconstruction is deterministic from the
    /// persisted metadata rather than depending on mode-setting escape
    /// sequences being present in the retained byte tail.
    pub fn restore_from_snapshot(&mut self, snap: &ScreenSnapshotV1) {
        let clean = crate::screen::restart_safe_scrollback(&snap.screen_bytes);
        self.screen.feed(clean);
        self.screen.restore_modes(&snap.modes);
        self.screen.restore_cursor_visible(snap.cursor_visible);
        self.output_seq = snap.pane_output_seq;
    }
}

/// Write scrollback data to disk: strip client queries, append, and rotate.
///
/// This is the I/O-only counterpart of [`Pane::flush_scrollback`]. It can
/// be called outside the server mutex after draining pending bytes with
/// [`Pane::take_pending_flush`].
pub fn write_scrollback_to_disk(path: &Path, data: &[u8]) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let clean = strip_client_queries(data);
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(&clean)?;
    rotate_scrollback_log(path, DEFAULT_MAX_SCROLLBACK_LOG, SCROLLBACK_ROTATE_KEEP)
}

/// Rotate a scrollback log file when it exceeds `max_bytes`.
///
/// Renames `log` → `log.1`, `log.1` → `log.2`, etc., keeping at most
/// `keep` rotated segments. The caller creates a fresh `log` on the next
/// append. This avoids the mid-escape-sequence corruption caused by the
/// old byte-boundary truncation approach (RFC-022 §4).
fn rotate_scrollback_log(path: &Path, max_bytes: u64, keep: u32) -> Result<(), std::io::Error> {
    let meta = std::fs::metadata(path)?;
    if meta.len() <= max_bytes {
        return Ok(());
    }

    // Shift existing segments: .3 → deleted, .2 → .3, .1 → .2
    for i in (1..keep).rev() {
        let src = path.with_extension(format!("log.{i}"));
        let dst = path.with_extension(format!("log.{}", i + 1));
        if src.exists() {
            std::fs::rename(&src, &dst)?;
        }
    }

    // Delete the oldest segment if it exceeds keep count.
    let oldest = path.with_extension(format!("log.{}", keep + 1));
    if oldest.exists() {
        std::fs::remove_file(&oldest)?;
    }

    // Current log → .1
    let first_rotated = path.with_extension("log.1");
    std::fs::rename(path, &first_rotated)?;

    Ok(())
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
    fn flush_scrollback_writes_to_runtime_dir_not_cache_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let runtime_id = Uuid::new_v4();
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"data");

        pane.flush_scrollback(tmp.path(), runtime_id).unwrap();

        let log_path = pane.scrollback_log_path.as_ref().unwrap();
        let expected = tmp
            .path()
            .join("runtimes")
            .join(runtime_id.to_string())
            .join("scrollback")
            .join(format!("{}.log", pane.id));
        assert_eq!(log_path, &expected);
        assert!(log_path.exists());
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
    fn flush_scrollback_rotates_when_exceeding_cap() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_id = Uuid::new_v4();
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);

        // Write more than DEFAULT_MAX_SCROLLBACK_LOG in multiple flushes.
        let chunk = vec![b'A'; 4 * 1024 * 1024]; // 4 MB
        for _ in 0..4 {
            pane.feed_output(&chunk);
            pane.flush_scrollback(tmp.path(), session_id).unwrap();
        }
        // After 16 MB written with 10 MB cap, rotation should have occurred.
        let log_path = pane.scrollback_log_path.as_ref().unwrap();
        let rotated = log_path.with_extension("log.1");
        assert!(rotated.exists(), "rotated segment .1 should exist");
    }

    #[test]
    fn rotate_scrollback_log_shifts_segments() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("scrollback").join("test.log");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();

        // Create a file exceeding the cap.
        std::fs::write(&path, vec![b'A'; 100]).unwrap();
        rotate_scrollback_log(&path, 50, 3).unwrap();

        // Original should be gone (renamed to .1).
        assert!(!path.exists());
        assert!(path.with_extension("log.1").exists());

        // Create a new log and rotate again.
        std::fs::write(&path, vec![b'B'; 100]).unwrap();
        rotate_scrollback_log(&path, 50, 3).unwrap();

        assert!(!path.exists());
        assert!(path.with_extension("log.1").exists());
        assert!(path.with_extension("log.2").exists());
    }

    #[test]
    fn rotate_scrollback_log_respects_keep_limit() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("test.log");

        for i in 0..6 {
            std::fs::write(&path, vec![b'A' + i; 100]).unwrap();
            rotate_scrollback_log(&path, 50, 3).unwrap();
        }

        // Should keep at most 3 rotated segments.
        assert!(path.with_extension("log.1").exists());
        assert!(path.with_extension("log.2").exists());
        assert!(path.with_extension("log.3").exists());
        assert!(!path.with_extension("log.4").exists());
    }

    #[test]
    fn rotate_scrollback_log_noop_when_under_cap() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("test.log");
        std::fs::write(&path, b"small").unwrap();
        rotate_scrollback_log(&path, 100, 3).unwrap();
        assert!(path.exists());
        assert!(!path.with_extension("log.1").exists());
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

    #[test]
    fn release_scrollback_clears_raw_bytes_and_pending_flush() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"hello world\r\nsome output\r\n");
        assert!(!pane.screen.raw_bytes().is_empty());
        assert!(pane.has_pending_flush());

        pane.release_scrollback();

        assert!(pane.screen.raw_bytes().is_empty());
        assert!(!pane.has_pending_flush());
    }

    #[test]
    fn pending_flush_capped_at_scrollback_limit() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        // Feed more than DEFAULT_MAX_SCROLLBACK (10 MB) without flushing.
        let chunk = vec![b'A'; 4 * 1024 * 1024];
        for _ in 0..4 {
            pane.feed_output(&chunk);
        }
        // pending_flush should be capped, not 16 MB.
        assert!(
            pane.pending_flush.len() <= DEFAULT_MAX_SCROLLBACK,
            "pending_flush {} exceeds cap {DEFAULT_MAX_SCROLLBACK}",
            pane.pending_flush.len()
        );
    }

    #[test]
    fn pending_flush_capacity_shrinks_after_flush() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_id = Uuid::new_v4();
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        // Grow pending_flush to a large size.
        let chunk = vec![b'B'; 2 * 1024 * 1024];
        pane.feed_output(&chunk);
        pane.flush_scrollback(tmp.path(), session_id).unwrap();
        // After flush, capacity should be released, not retained at 2 MB.
        let cap = pane.pending_flush.capacity();
        assert!(cap < 1024 * 1024, "pending_flush capacity {cap} should shrink after flush");
    }

    #[test]
    fn release_scrollback_is_idempotent() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"data");
        pane.release_scrollback();
        pane.release_scrollback();
        assert!(pane.screen.raw_bytes().is_empty());
    }

    #[test]
    fn flush_scrollback_strips_dsr_queries() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_id = Uuid::new_v4();
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);

        // Feed output containing DSR queries mixed with real content.
        pane.feed_output(b"line1\r\n\x1b[6nline2\r\n\x1b[c\x1b[>c");
        pane.flush_scrollback(tmp.path(), session_id).unwrap();

        let log_path = pane.scrollback_log_path.as_ref().unwrap();
        let content = std::fs::read(log_path).unwrap();
        // Scrollback log should not contain DSR/DA1/DA2 query sequences.
        assert_eq!(content, b"line1\r\nline2\r\n");
    }

    #[test]
    fn to_screen_snapshot_captures_pane_state() {
        let mut pane = Pane::new(Uuid::new_v4(), 120, 40);
        pane.feed_output(b"\x1b]0;my-title\x07");
        pane.feed_output(b"\x1b]7;file://localhost/home/user\x07");
        pane.feed_output(b"hello world\r\n");
        pane.feed_output(b"\x1b[?2004h"); // bracketed paste

        let snap = pane.to_screen_snapshot();
        assert_eq!(snap.pane_id, pane.id);
        assert_eq!(snap.cols, 120);
        assert_eq!(snap.rows, 40);
        assert_eq!(snap.title.as_deref(), Some("my-title"));
        assert_eq!(snap.cwd.as_deref(), Some("/home/user"));
        assert!(snap.modes.bracketed_paste);
        assert!(!snap.screen_bytes.is_empty());
        assert_eq!(snap.schema_version, SCREEN_SNAPSHOT_SCHEMA_VERSION);
    }

    #[test]
    fn to_screen_snapshot_captures_cursor_position() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"line1\r\nline2\r\nXYZ");

        let snap = pane.to_screen_snapshot();
        assert_eq!(snap.cursor_row, 2);
        assert_eq!(snap.cursor_col, 3);
    }

    #[test]
    fn to_screen_snapshot_captures_all_terminal_modes() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"\x1b[?2004h"); // bracketed paste
        pane.feed_output(b"\x1b[?1h"); // application cursor keys
        pane.feed_output(b"\x1b="); // application keypad
        pane.feed_output(b"\x1b[?1003h"); // any-event mouse
        pane.feed_output(b"\x1b[?1006h"); // SGR mouse
        pane.feed_output(b"\x1b[?1004h"); // focus reporting
        pane.feed_output(b"\x1b[?25l"); // hide cursor

        let snap = pane.to_screen_snapshot();
        assert!(snap.modes.bracketed_paste);
        assert!(snap.modes.application_cursor_keys);
        assert!(snap.modes.application_keypad);
        assert_eq!(snap.modes.mouse_tracking_mode, 1003);
        assert!(snap.modes.sgr_mouse);
        assert!(snap.modes.focus_reporting);
        assert!(!snap.cursor_visible);
    }

    #[test]
    fn to_screen_snapshot_screen_bytes_bounded_by_max_snapshot() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        // Feed more than MAX_SNAPSHOT_BYTES.
        let big = vec![b'X'; MAX_SNAPSHOT_BYTES + 1024];
        pane.feed_output(&big);

        let snap = pane.to_screen_snapshot();
        assert!(snap.screen_bytes.len() <= MAX_SNAPSHOT_BYTES);
    }

    #[test]
    fn to_screen_snapshot_round_trips_through_json() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"test data\r\n");

        let snap = pane.to_screen_snapshot();
        let json = serde_json::to_string_pretty(&snap).unwrap();
        let recovered: ScreenSnapshotV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(snap, recovered);
    }

    // ── no_persist tests ────────────────────────────────────────────

    #[test]
    fn no_persist_pane_skips_scrollback_flush() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_id = Uuid::new_v4();
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.no_persist = true;
        pane.feed_output(b"secret data");
        assert!(pane.has_pending_flush());

        pane.flush_scrollback(tmp.path(), session_id).unwrap();

        assert!(!pane.has_pending_flush());
        assert!(pane.scrollback_log_path.is_none());
        // No file should have been created.
        let scrollback_dir = tmp.path().join("scrollback");
        assert!(!scrollback_dir.exists());
    }

    #[test]
    fn no_persist_pane_snapshot_is_confidential() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.no_persist = true;
        pane.feed_output(b"hello");

        let snap = pane.to_screen_snapshot();
        assert!(snap.confidential);
    }

    #[test]
    fn normal_pane_snapshot_is_not_confidential() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"hello");

        let snap = pane.to_screen_snapshot();
        assert!(!snap.confidential);
    }

    #[test]
    fn no_persist_defaults_to_false() {
        let pane = Pane::new(Uuid::new_v4(), 80, 24);
        assert!(!pane.no_persist);
    }

    // ── restore_from_snapshot tests ─────────────────────────────────

    fn snapshot_with_modes(pane_id: Uuid) -> ScreenSnapshotV1 {
        ScreenSnapshotV1 {
            schema_version: SCREEN_SNAPSHOT_SCHEMA_VERSION,
            pane_id,
            cols: 80,
            rows: 24,
            cursor_row: 5,
            cursor_col: 10,
            cursor_visible: false,
            title: None,
            cwd: None,
            pane_output_seq: 42,
            modes: TerminalModeSnapshot {
                bracketed_paste: true,
                application_cursor_keys: true,
                application_keypad: true,
                mouse_tracking_mode: 1003,
                sgr_mouse: true,
                focus_reporting: true,
            },
            screen_bytes: b"line1\r\nline2\r\n".to_vec(),
            confidential: false,
        }
    }

    #[test]
    fn restore_from_snapshot_restores_modes() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        let snap = snapshot_with_modes(pane.id);

        pane.restore_from_snapshot(&snap);

        assert!(pane.screen.bracketed_paste_mode());
        assert!(pane.screen.application_cursor_keys());
        assert!(pane.screen.application_keypad());
        assert_eq!(pane.screen.mouse_tracking_mode(), 1003);
        assert!(pane.screen.sgr_mouse_mode());
        assert!(pane.screen.focus_event_mode());
    }

    #[test]
    fn restore_from_snapshot_restores_cursor_visibility() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        assert!(pane.screen.cursor_visible());

        let snap = snapshot_with_modes(pane.id);
        pane.restore_from_snapshot(&snap);

        assert!(!pane.screen.cursor_visible());
    }

    #[test]
    fn restore_from_snapshot_restores_output_seq() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        assert_eq!(pane.output_seq, 0);

        let snap = snapshot_with_modes(pane.id);
        pane.restore_from_snapshot(&snap);

        assert_eq!(pane.output_seq, 42);
    }

    #[test]
    fn restore_from_snapshot_feeds_screen_bytes() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        let snap = snapshot_with_modes(pane.id);

        pane.restore_from_snapshot(&snap);

        assert!(!pane.screen.raw_bytes().is_empty());
    }

    #[test]
    fn restore_from_snapshot_modes_override_replay() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        // screen_bytes enable bracketed paste, but snapshot metadata says off.
        let snap = ScreenSnapshotV1 {
            schema_version: SCREEN_SNAPSHOT_SCHEMA_VERSION,
            pane_id: pane.id,
            cols: 80,
            rows: 24,
            cursor_row: 0,
            cursor_col: 0,
            cursor_visible: true,
            title: None,
            cwd: None,
            pane_output_seq: 0,
            modes: TerminalModeSnapshot {
                bracketed_paste: false,
                application_cursor_keys: false,
                application_keypad: false,
                mouse_tracking_mode: 0,
                sgr_mouse: false,
                focus_reporting: false,
            },
            screen_bytes: b"\x1b[?2004h\x1b[?1hsome text\r\n".to_vec(),
            confidential: false,
        };

        pane.restore_from_snapshot(&snap);

        // Metadata wins over replay-derived state.
        assert!(!pane.screen.bracketed_paste_mode());
        assert!(!pane.screen.application_cursor_keys());
    }

    // ── feed_cleanup tests ──────────────────────────────────────────

    #[test]
    fn feed_cleanup_resets_dirty_screen_modes() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"\x1b[?1049h"); // alt-screen
        pane.feed_output(b"\x1b[?25l"); // hide cursor
        pane.feed_output(b"\x1b[?1003h"); // any-event mouse
        pane.feed_output(b"\x1b[?1006h"); // SGR mouse
        pane.feed_output(b"\x1b[?1004h"); // focus reporting
        pane.feed_output(b"\x1b[?1h"); // application cursor keys
        pane.feed_output(b"\x1b="); // application keypad
        pane.feed_output(b"\x1b[?2004h"); // bracketed paste

        pane.feed_cleanup();

        assert!(!pane.screen.alternate_screen());
        assert!(pane.screen.cursor_visible());
        assert_eq!(pane.screen.mouse_tracking_mode(), 0);
        assert!(!pane.screen.sgr_mouse_mode());
        assert!(!pane.screen.focus_event_mode());
        assert!(!pane.screen.application_cursor_keys());
        assert!(!pane.screen.application_keypad());
        assert!(!pane.screen.bracketed_paste_mode());
    }

    #[test]
    fn feed_cleanup_appends_to_pending_flush() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"hello");
        let before = pane.pending_flush_len();

        pane.feed_cleanup();

        let cleanup_len = crate::screen::terminal_cleanup_bytes().len();
        assert_eq!(pane.pending_flush_len(), before + cleanup_len);
    }

    #[test]
    fn feed_cleanup_increments_output_seq() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        let before = pane.output_seq;
        pane.feed_cleanup();
        assert_eq!(pane.output_seq, before + 1);
    }

    #[test]
    fn feed_cleanup_returns_cleanup_bytes() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        let returned = pane.feed_cleanup();
        assert_eq!(returned, crate::screen::terminal_cleanup_bytes());
    }

    #[test]
    fn feed_cleanup_is_idempotent_on_clean_state() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"hello\r\n");

        pane.feed_cleanup();
        pane.feed_cleanup();

        assert!(pane.screen.cursor_visible());
        assert_eq!(pane.screen.mouse_tracking_mode(), 0);
        assert!(!pane.screen.alternate_screen());
    }

    #[test]
    fn snapshot_after_cleanup_shows_clean_modes() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"\x1b[?1049h\x1b[?25l\x1b[?1003h\x1b[?2004h");

        pane.feed_cleanup();

        let snap = pane.to_screen_snapshot();
        assert!(snap.cursor_visible);
        assert!(!snap.modes.bracketed_paste);
        assert_eq!(snap.modes.mouse_tracking_mode, 0);
        assert!(!snap.modes.application_cursor_keys);
    }

    #[test]
    fn cleanup_bytes_flushed_to_scrollback_log() {
        let tmp = tempfile::TempDir::new().unwrap();
        let session_id = Uuid::new_v4();
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"hello");
        pane.feed_cleanup();
        pane.flush_scrollback(tmp.path(), session_id).unwrap();

        let log_path = pane.scrollback_log_path.as_ref().unwrap();
        let content = std::fs::read(log_path).unwrap();
        let cleanup = crate::screen::terminal_cleanup_bytes();
        assert!(content.ends_with(cleanup), "scrollback log should end with cleanup bytes");
    }

    // ── take_pending_flush / write_scrollback_to_disk tests ─────────

    #[test]
    fn take_pending_flush_drains_buffer() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"hello world");
        assert!(pane.has_pending_flush());

        let data = pane.take_pending_flush();
        assert_eq!(data, b"hello world");
        assert!(!pane.has_pending_flush());
        assert_eq!(pane.pending_flush_len(), 0);
    }

    #[test]
    fn take_pending_flush_returns_empty_when_nothing_pending() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        let data = pane.take_pending_flush();
        assert!(data.is_empty());
    }

    #[test]
    fn write_scrollback_to_disk_creates_file_and_strips_queries() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("scrollback").join("test.log");
        let data = b"line1\r\n\x1b[6nline2\r\n";

        write_scrollback_to_disk(&path, data).unwrap();

        let content = std::fs::read(&path).unwrap();
        assert_eq!(content, b"line1\r\nline2\r\n");
    }

    #[test]
    fn write_scrollback_to_disk_appends_to_existing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("test.log");

        write_scrollback_to_disk(&path, b"first ").unwrap();
        write_scrollback_to_disk(&path, b"second").unwrap();

        let content = std::fs::read(&path).unwrap();
        assert_eq!(content, b"first second");
    }

    #[test]
    fn write_scrollback_to_disk_rotates_large_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("test.log");

        let chunk = vec![b'A'; 4 * 1024 * 1024];
        for _ in 0..4 {
            write_scrollback_to_disk(&path, &chunk).unwrap();
        }

        let rotated = path.with_extension("log.1");
        assert!(rotated.exists(), "rotated segment .1 should exist");
    }

    // ── two-phase accept_output / parse_and_extract tests ───────────

    #[test]
    fn accept_output_stores_raw_bytes_without_vte_parsing() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        let osc7 = b"\x1b]7;file://localhost/tmp/test\x07";
        pane.accept_output(osc7);

        // Raw bytes stored in screen.
        assert_eq!(pane.screen.raw_bytes(), osc7);
        // Pending flush updated.
        assert!(pane.has_pending_flush());
        // Seq incremented.
        assert_eq!(pane.output_seq, 1);
        // VTE not parsed — CWD not extracted yet.
        assert!(pane.cwd.is_none());
        assert!(pane.screen.cwd().is_none());
    }

    #[test]
    fn parse_and_extract_runs_vte_after_accept() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        let osc7 = b"\x1b]7;file://localhost/tmp/test\x07";
        pane.accept_output(osc7);
        let result = pane.parse_and_extract(osc7);

        assert_eq!(result.new_cwd.as_deref(), Some("/tmp/test"));
        assert_eq!(pane.cwd.as_deref(), Some("/tmp/test"));
    }

    #[test]
    fn take_screen_and_return_screen_equivalent_to_feed_output() {
        // feed_output path
        let mut pane_a = Pane::new(Uuid::new_v4(), 80, 24);
        let data = b"\x1b]0;my-title\x07\x1b]7;file://localhost/home/user\x07hello\r\n\x1b[6n";
        let result_a = pane_a.feed_output(data);

        // two-phase path
        let mut pane_b = Pane::new(Uuid::new_v4(), 80, 24);
        pane_b.accept_output(data);
        let mut screen = pane_b.take_screen();
        screen.parse(data);
        let result_b = pane_b.return_screen(screen);

        assert_eq!(result_a.new_cwd, result_b.new_cwd);
        assert_eq!(result_a.new_title, result_b.new_title);
        assert_eq!(result_a.pending_replies, result_b.pending_replies);
        assert_eq!(pane_a.cwd, pane_b.cwd);
        assert_eq!(pane_a.title, pane_b.title);
        assert_eq!(pane_a.output_seq, pane_b.output_seq);
    }

    #[test]
    fn take_screen_returns_screen_with_accumulated_state() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"line1\r\nline2\r\n");
        let screen = pane.take_screen();
        assert_eq!(screen.cursor_position(), (2, 0));
        assert!(!screen.raw_bytes().is_empty());
    }

    #[test]
    fn return_screen_restores_screen_state() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"hello");

        let mut screen = pane.take_screen();
        // Pane now has a fresh empty screen.
        assert!(pane.screen.raw_bytes().is_empty());

        screen.parse(b" world");
        pane.return_screen(screen);

        // Screen is restored with all accumulated state.
        assert!(!pane.screen.raw_bytes().is_empty());
        assert_eq!(pane.screen.cursor_position(), (0, 11));
    }

    #[test]
    fn accept_output_increments_seq_each_call() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        assert_eq!(pane.output_seq, 0);
        pane.accept_output(b"a");
        assert_eq!(pane.output_seq, 1);
        pane.accept_output(b"b");
        assert_eq!(pane.output_seq, 2);
    }
}
