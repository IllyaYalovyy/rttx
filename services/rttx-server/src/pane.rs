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

/// Workspace state of a single pane.
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
            screen: PaneScreen::new_sized(DEFAULT_MAX_SCROLLBACK, cols, rows),
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
    /// buffer. The screen receives raw bytes for accurate VTE parsing,
    /// while `pending_flush` receives stripped bytes (terminal query
    /// sequences removed) so that `flush_scrollback` can write directly
    /// without re-stripping. The fast path (no ESC byte) avoids any
    /// allocation beyond the `extend_from_slice` memcpy.
    ///
    /// Call [`parse_and_extract`] afterwards (potentially after releasing
    /// the lock) to run the expensive VTE state machine.
    pub fn accept_output(&mut self, data: &[u8]) {
        self.screen.accept_raw(data);
        if data.contains(&0x1b) {
            self.pending_flush.extend_from_slice(&strip_client_queries(data));
        } else {
            self.pending_flush.extend_from_slice(data);
        }
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

    /// Whether an interactive shell is the foreground process of the pane's
    /// terminal — that is, nothing but a shell prompt is running in it.
    ///
    /// Read from `/proc`: the pane's child names the terminal's foreground
    /// process group (`tpgid` in its `stat`), and that group's leader must
    /// be a known shell. A pane whose root process is itself a full-screen
    /// app (`htop`, a one-off command, an exerciser) is therefore never
    /// mistaken for an idle shell. `None` when the pane has no live child
    /// or the kernel will not say.
    #[must_use]
    pub fn foreground_is_shell(&self) -> Option<bool> {
        let pid = self.child_pid?;
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let tpgid = foreground_pgrp_from_stat(&stat)?;
        if tpgid <= 0 {
            return Some(false);
        }
        let comm = std::fs::read_to_string(format!("/proc/{tpgid}/comm")).ok()?;
        Some(is_interactive_shell_name(comm.trim()))
    }

    /// Clear terminal modes that only a running full-screen app uses when
    /// the shell itself is in the foreground, so a client attaching to a
    /// pane whose app died mid-flight gets a usable terminal rather than
    /// one that needs `reset`: no stuck mouse tracking spraying escape
    /// codes on click, no alternate screen, no invisible cursor.
    ///
    /// Returns the cleanup bytes that were applied (and should reach any
    /// client already attached), or `None` when nothing needed doing.
    pub fn sanitize_modes_if_idle(&mut self) -> Option<Vec<u8>> {
        if self.is_exited() || !self.screen.has_app_only_modes() {
            return None;
        }
        if self.foreground_is_shell() != Some(true) {
            return None;
        }
        let cleanup = self.screen.cleanup_sequence(true);
        self.screen.feed(&cleanup);
        Some(cleanup)
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
    pub fn feed_cleanup(&mut self) -> Vec<u8> {
        let cleanup = self.screen.cleanup_sequence(false);
        self.screen.feed(&cleanup);
        self.pending_flush.extend_from_slice(&cleanup);
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
    ///
    /// `screen_bytes` is a rendering of the pane's primary buffer (see
    /// [`PaneScreen::primary_buffer_stream`]), not a tail of raw output, so
    /// a daemon restart rebuilds the pane from a clean description of what
    /// was on screen rather than from a byte-stream suffix.
    #[must_use]
    pub fn to_screen_snapshot(&mut self) -> ScreenSnapshotV1 {
        let (cursor_row, cursor_col) = self.screen.cursor_position();
        let mut screen_bytes = self.screen.primary_buffer_stream();
        crate::screen::drop_oldest_lines_over(&mut screen_bytes, MAX_SNAPSHOT_BYTES);
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
                alternate_screen: self.screen.alternate_screen(),
            },
            screen_bytes,
            confidential: self.no_persist,
        }
    }

    /// Restore canonical pane state from a persisted screen snapshot.
    ///
    /// A daemon restart destroyed the process that owned this pane. Any
    /// full-screen TUI (Claude, Codex, vim, htop, …) that had put the terminal
    /// into alternate-screen / mouse-tracking / bracketed-paste mode is gone,
    /// and the pane is about to be handed to a freshly respawned shell.
    /// Faithfully restoring the dead app's modes leaves that shell in a broken
    /// input state — most visibly, stuck mouse tracking makes every pointer
    /// movement inject `\x1b[<btn;col;rowM` reports onto the command line, and a
    /// replayed alt-screen frame scatters absolutely-positioned text across the
    /// buffer. So reconstruction is treated like a clean process exit: the
    /// transient app frame is dropped and the terminal is reset to an
    /// interactive baseline via [`terminal_cleanup_bytes`].
    ///
    /// The reset is applied to the screen state itself, so it also propagates to
    /// clients that attach after reconstruction — their snapshot is rebuilt from
    /// this screen's live mode flags and byte tail.
    ///
    /// [`terminal_cleanup_bytes`]: crate::screen::terminal_cleanup_bytes
    pub fn restore_from_snapshot(&mut self, snap: &ScreenSnapshotV1) {
        // A TUI owned the screen if it was in the alternate buffer or had mouse
        // tracking enabled. Its on-screen content is transient app UI, not
        // scrollback worth replaying.
        let tui_owned_screen = snap.modes.alternate_screen || snap.modes.mouse_tracking_mode != 0;

        if !tui_owned_screen {
            // Normal shell: the retained tail is real scrollback worth showing.
            let clean = crate::screen::restart_safe_scrollback(&snap.screen_bytes);
            self.screen.feed(clean);
        }

        // Reset alt-screen, mouse tracking, bracketed paste, application cursor
        // keys, and cursor visibility to a sane baseline. Feeding the cleanup
        // bytes through the parser also clears the corresponding mode flags, so
        // the reconstructed screen reports a clean state to attaching clients.
        let cleanup = self.screen.cleanup_sequence(false);
        self.screen.feed(&cleanup);
        self.screen.move_cursor_below_content();

        self.output_seq = snap.pane_output_seq;
    }
}

/// The terminal's foreground process group (`tpgid`, field 8) from a
/// `/proc/<pid>/stat` line, counted after the parenthesised command name,
/// which may itself contain spaces and parentheses.
#[must_use]
pub fn foreground_pgrp_from_stat(stat: &str) -> Option<i64> {
    let after_comm = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    // after_comm starts at field 3 (state); tpgid is index 5.
    fields.get(5)?.parse().ok()
}

/// Whether `comm` names an interactive shell: the only kind of foreground
/// process that means "nothing is running here".
#[must_use]
pub fn is_interactive_shell_name(comm: &str) -> bool {
    let name = comm.trim_start_matches('-');
    matches!(
        name,
        "bash"
            | "zsh"
            | "fish"
            | "sh"
            | "dash"
            | "ksh"
            | "mksh"
            | "tcsh"
            | "csh"
            | "nu"
            | "elvish"
            | "xonsh"
            | "ash"
            | "oil"
            | "osh"
    )
}

/// Write scrollback data to disk: append and rotate.
///
/// Data is expected to be pre-stripped (terminal query sequences already
/// removed by [`Pane::accept_output`]). This is the I/O-only counterpart
/// of [`Pane::flush_scrollback`]. It can be called outside the server
/// mutex after draining pending bytes with [`Pane::take_pending_flush`].
pub fn write_scrollback_to_disk(path: &Path, data: &[u8]) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(data)?;
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
            .join("workspaces")
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
    fn read_proc_cwd_returns_none_without_pid() {
        let pane = Pane::new(Uuid::new_v4(), 80, 24);
        assert!(pane.read_proc_cwd().is_none());
    }

    #[test]
    fn read_proc_cwd_reads_current_process_cwd() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.child_pid = Some(std::process::id());
        let cwd = pane.read_proc_cwd();
        assert!(cwd.is_some(), "/proc/self/cwd should be readable");
    }

    #[test]
    fn proc_cwd_poll_updates_pane_cwd_when_different() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.child_pid = Some(std::process::id());
        assert!(pane.cwd.is_none());

        // Simulate the poll: read_proc_cwd detects a new value.
        let proc_cwd = pane.read_proc_cwd();
        assert!(proc_cwd.is_some());
        if let Some(ref new_cwd) = proc_cwd
            && pane.cwd.as_deref() != Some(new_cwd.as_str())
        {
            pane.cwd = Some(new_cwd.clone());
        }
        assert!(pane.cwd.is_some());
    }

    #[test]
    fn proc_cwd_poll_no_update_when_same() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.child_pid = Some(std::process::id());
        let current = pane.read_proc_cwd().unwrap();
        pane.cwd = Some(current.clone());

        // When proc CWD matches stored CWD, no update needed.
        let proc_cwd = pane.read_proc_cwd();
        assert_eq!(proc_cwd.as_deref(), Some(current.as_str()));
        assert_eq!(pane.cwd.as_deref(), proc_cwd.as_deref());
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
                alternate_screen: false,
            },
            screen_bytes: b"line1\r\nline2\r\n".to_vec(),
            confidential: false,
        }
    }

    /// Return a normal-shell snapshot: no alt-screen, no mouse tracking, with
    /// real scrollback in `screen_bytes`.
    fn normal_shell_snapshot(pane_id: Uuid, screen_bytes: &[u8]) -> ScreenSnapshotV1 {
        let mut snap = snapshot_with_modes(pane_id);
        snap.modes = TerminalModeSnapshot {
            bracketed_paste: false,
            application_cursor_keys: false,
            application_keypad: false,
            mouse_tracking_mode: 0,
            sgr_mouse: false,
            focus_reporting: false,
            alternate_screen: false,
        };
        snap.screen_bytes = screen_bytes.to_vec();
        snap
    }

    #[test]
    fn restore_from_snapshot_resets_tui_modes_to_baseline() {
        // Regression: a daemon restart kills the TUI (Claude/Codex/vim) that
        // enabled mouse tracking. Reconstruction must NOT restore those modes —
        // otherwise the respawned shell inherits mouse tracking and every
        // pointer movement injects `\x1b[<btn;col;rowM` reports onto the prompt.
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        let snap = snapshot_with_modes(pane.id); // mouse_tracking_mode: 1003

        pane.restore_from_snapshot(&snap);

        assert_eq!(pane.screen.mouse_tracking_mode(), 0, "mouse tracking must be reset");
        assert!(!pane.screen.sgr_mouse_mode());
        assert!(!pane.screen.bracketed_paste_mode());
        assert!(!pane.screen.application_cursor_keys());
        assert!(!pane.screen.application_keypad());
        assert!(!pane.screen.focus_event_mode());
        assert!(!pane.screen.alternate_screen());
    }

    #[test]
    fn restore_from_snapshot_forces_cursor_visible() {
        // TUI apps often hide the cursor; the reconstructed baseline shows it so
        // the fresh shell prompt is usable.
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        let snap = snapshot_with_modes(pane.id); // cursor_visible: false

        pane.restore_from_snapshot(&snap);

        assert!(pane.screen.cursor_visible());
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
    fn restore_from_snapshot_skips_transient_frame_for_tui_app() {
        // The alt-screen / mouse-driven frame is transient app UI, not
        // scrollback. Replaying its absolute-positioned redraw bytes would
        // scatter text across the reconstructed buffer, so it must be dropped.
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        let snap = snapshot_with_modes(pane.id); // mouse 1003, bytes "line1\r\nline2\r\n"

        pane.restore_from_snapshot(&snap);

        assert!(
            !pane.screen.raw_bytes().windows(5).any(|w| w == b"line1"),
            "transient TUI frame should not be replayed"
        );
    }

    #[test]
    fn restore_from_snapshot_detects_tui_via_alternate_screen() {
        // A full-screen app with mouse tracking off but the alternate buffer
        // active (e.g. `less`, `man`) is still transient — skip its frame.
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        let mut snap = snapshot_with_modes(pane.id);
        snap.modes.mouse_tracking_mode = 0;
        snap.modes.alternate_screen = true;

        pane.restore_from_snapshot(&snap);

        assert!(!pane.screen.raw_bytes().windows(5).any(|w| w == b"line1"));
    }

    #[test]
    fn restore_from_snapshot_replays_scrollback_for_normal_shell() {
        // A normal shell (no alt-screen, no mouse) has real scrollback worth
        // showing on reconnect.
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        let snap = normal_shell_snapshot(pane.id, b"hello world\r\n$ \r\n");

        pane.restore_from_snapshot(&snap);

        assert!(
            pane.screen.raw_bytes().windows(11).any(|w| w == b"hello world"),
            "normal-shell scrollback should be replayed"
        );
    }

    #[test]
    fn restore_from_snapshot_resets_modes_left_by_replayed_bytes() {
        // Even for a normal-shell replay, mode-setting sequences embedded in the
        // retained bytes (bracketed paste, application cursor keys) must be
        // neutralized so the reconstructed baseline is clean.
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        let snap = normal_shell_snapshot(pane.id, b"\x1b[?2004h\x1b[?1hsome text\r\n");

        pane.restore_from_snapshot(&snap);

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
        // In the alternate screen the sequence leaves it first.
        let mut tui = Pane::new(Uuid::new_v4(), 80, 24);
        tui.feed_output(b"\x1b[?1049h");
        let returned = tui.feed_cleanup();
        assert!(returned.starts_with(crate::screen::leave_alternate_screen_bytes()));
        assert!(!tui.screen.alternate_screen());
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
    fn write_scrollback_to_disk_writes_pre_stripped_data() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("scrollback").join("test.log");
        let data = b"line1\r\nline2\r\n";

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
    fn accept_output_strips_queries_from_pending_flush() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        let data = b"hello\x1b[6nworld\x1b[c";
        pane.accept_output(data);

        // Screen gets raw bytes (needed for accurate VTE parsing).
        assert_eq!(pane.screen.raw_bytes(), data);
        // Pending flush gets stripped bytes (no double strip on disk write).
        let flushed = pane.take_pending_flush();
        assert_eq!(flushed, b"helloworld");
    }

    #[test]
    fn accept_output_pending_flush_unchanged_without_queries() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        let data = b"plain text output\r\n";
        pane.accept_output(data);

        let flushed = pane.take_pending_flush();
        assert_eq!(flushed, data);
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
    #[test]
    fn foreground_pgrp_is_read_from_stat() {
        // pid (comm) state ppid pgrp session tty_nr tpgid ...
        let shell_idle = "4242 (bash) S 1 4242 4242 34816 4242 4194560 0 0 0 0";
        assert_eq!(foreground_pgrp_from_stat(shell_idle), Some(4242));
        let app_running = "4242 (bash) S 1 4242 4242 34816 5000 4194560 0 0 0 0";
        assert_eq!(foreground_pgrp_from_stat(app_running), Some(5000));
        let odd_comm = "4242 (my (weird) sh) S 1 4242 4242 34816 4242 0";
        assert_eq!(foreground_pgrp_from_stat(odd_comm), Some(4242));
        assert_eq!(foreground_pgrp_from_stat("garbage"), None);
    }

    #[test]
    fn only_interactive_shells_count_as_idle_foreground() {
        for shell in ["bash", "-bash", "zsh", "fish", "sh", "dash", "nu"] {
            assert!(is_interactive_shell_name(shell), "{shell}");
        }
        for app in ["htop", "vim", "pty-exerciser", "ssh", "claude", "less", "python3"] {
            assert!(!is_interactive_shell_name(app), "{app}");
        }
    }

    #[test]
    fn idle_sanitizer_leaves_a_pane_without_app_modes_alone() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.child_pid = Some(std::process::id());
        // (This test binary is not a shell, so even with app modes armed
        // the sanitizer would decline; here it must decline earlier, on
        // the absence of app modes, without touching the shell's modes.)
        pane.feed_output(b"\x1b[?2004h\x1b[?1h$ ");
        assert!(pane.sanitize_modes_if_idle().is_none(), "shell modes are not app modes");
        assert!(pane.screen.bracketed_paste_mode());
        assert!(pane.screen.application_cursor_keys());
    }

    #[test]
    fn idle_sanitizer_needs_a_live_child_to_judge() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        pane.feed_output(b"\x1b[?1000h\x1b[?1049h");
        assert!(pane.sanitize_modes_if_idle().is_none(), "no child: cannot tell, do nothing");
        assert_eq!(pane.screen.mouse_tracking_mode(), 1000);
    }

    #[test]
    fn idle_sanitizer_clears_app_modes_when_the_shell_is_foreground() {
        let mut pane = Pane::new(Uuid::new_v4(), 80, 24);
        // The foreground check needs a real shell in /proc; the end-to-end
        // case lives in tests/idle_mode_recovery.rs. Here, exercise the
        // screen half: what the cleanup does to the tracked modes.
        pane.feed_output(b"$ app\r\n\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[?25l\x1b[?1004h");
        assert!(pane.screen.has_app_only_modes());
        let cleanup = pane.screen.cleanup_sequence(true);
        assert!(cleanup.starts_with(crate::screen::leave_alternate_screen_bytes()));
        pane.screen.feed(&cleanup);
        assert!(!pane.screen.has_app_only_modes());
        assert!(!pane.screen.alternate_screen());
        assert_eq!(pane.screen.mouse_tracking_mode(), 0);
        assert!(!pane.screen.sgr_mouse_mode());
        assert!(!pane.screen.focus_event_mode());
        assert!(pane.screen.cursor_visible());
        // The rendered attach stream then arms nothing.
        let stream = pane.screen.reattach_stream();
        assert!(
            !stream.windows(4).any(|w| w == b"\x1b[?1"),
            "no DEC private modes armed: {stream:?}"
        );
    }

    /// An app that draws inline (a chat UI with a status line under the
    /// input box) leaves its cursor inside its frame. After a restart the
    /// app is gone, so the persisted rendering must not restore that cursor:
    /// the respawned shell's prompt goes after the last line, never over
    /// the frame.
    #[test]
    fn restart_reconstruction_ignores_a_dead_apps_cursor_inside_its_frame() {
        let mut before = Pane::new(Uuid::new_v4(), 80, 10);
        before.feed_output(b"$ claude\r\n> type here\r\n\r\n  model high | ~/proj\r\n");
        // Cursor back up into the input box, after "> ".
        before.feed_output(b"\x1b[3A\x1b[3G");
        assert_eq!(before.screen.cursor_position(), (1, 2));

        let snap = before.to_screen_snapshot();
        let mut after = Pane::new(snap.pane_id, snap.cols, snap.rows);
        after.restore_from_snapshot(&snap);
        after.feed_output(b"PROMPT> ");
        let mut client = vt100::Parser::new(10, 80, 100);
        client.process(&after.screen.reattach_stream());
        let rows: Vec<String> =
            client.screen().rows(0, 80).map(|r| r.trim_end().to_string()).collect();
        assert_eq!(rows[0], "$ claude");
        assert_eq!(rows[1], "> type here");
        // The status line was the unterminated last line and is dropped, as
        // a stale prompt would be; the new prompt follows the frame.
        let prompt_row = rows.iter().position(|r| r == "PROMPT>").expect("prompt drawn");
        assert!(prompt_row > 1, "prompt below the input box: {rows:?}");
        assert_eq!(client.screen().cursor_position(), (prompt_row as u16, 8));

        // A snapshot written by a 1.1.0 daemon is the raw byte stream, with
        // the app's cursor-up sequences in it; restoring it lands the cursor
        // inside the frame. The prompt must still go below the frame.
        let mut legacy = snap.clone();
        legacy.screen_bytes =
            b"$ claude\r\n> type here\r\n\r\n  model high | ~/proj\r\n\x1b[3A\x1b[3G".to_vec();
        let mut after = Pane::new(legacy.pane_id, legacy.cols, legacy.rows);
        after.restore_from_snapshot(&legacy);
        after.feed_output(b"PROMPT> ");
        let mut client = vt100::Parser::new(10, 80, 100);
        client.process(&after.screen.reattach_stream());
        let rows: Vec<String> =
            client.screen().rows(0, 80).map(|r| r.trim_end().to_string()).collect();
        assert_eq!(rows[1], "> type here", "the frame is not overwritten: {rows:?}");
        let prompt_row = rows.iter().position(|r| r == "PROMPT>").expect("prompt drawn");
        assert!(prompt_row >= 3, "prompt below the frame: {rows:?}");
        assert_eq!(client.screen().cursor_position(), (prompt_row as u16, 8));
    }

    /// A daemon restart rebuilds the pane from the persisted rendering and
    /// the respawned shell's prompt lands below the restored history.
    #[test]
    fn restart_reconstruction_places_the_new_prompt_below_restored_history() {
        let mut before = Pane::new(Uuid::new_v4(), 80, 24);
        before.feed_output(b"PROMPT> echo cycle-0\r\ncycle-0\r\nPROMPT> ");
        let snap = before.to_screen_snapshot();
        // Trailing blanks are not content; the old prompt line is dropped on
        // restore anyway (the process that printed it is gone).
        assert_eq!(
            String::from_utf8_lossy(&snap.screen_bytes),
            "PROMPT> echo cycle-0\r\ncycle-0\r\nPROMPT>"
        );

        let mut after = Pane::new(snap.pane_id, snap.cols, snap.rows);
        after.restore_from_snapshot(&snap);
        assert_eq!(
            after.screen.cursor_position(),
            (2, 0),
            "old prompt line dropped, cursor below history"
        );
        after.feed_output(b"PROMPT> ");
        assert_eq!(
            String::from_utf8_lossy(&after.screen.reattach_stream()),
            "PROMPT> echo cycle-0\r\ncycle-0\r\nPROMPT> "
        );
    }
}
