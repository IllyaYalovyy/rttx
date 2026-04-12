//! Canonical terminal screen state per pane.
//!
//! Feeds raw PTY output through `vte::Parser` to maintain an in-memory
//! representation of the terminal grid. Used to build snapshots for client
//! reconnection.

/// In-memory terminal screen state built from VTE parsing.
pub struct PaneScreen {
    parser: vte::Parser,
    performer: ScreenPerformer,
}

/// Implements `vte::Perform` to track cursor position and collect raw bytes.
///
/// A full cell-grid model is deferred to a later iteration. For now we store
/// the raw byte stream so snapshots can replay it to reconstruct state.
struct ScreenPerformer {
    /// Raw bytes received from the PTY, used for snapshot replay.
    raw_bytes: Vec<u8>,
    /// Maximum raw bytes to retain (scrollback cap).
    max_bytes: usize,
    /// Current cursor row (0-based).
    cursor_row: usize,
    /// Current cursor col (0-based).
    cursor_col: usize,
    /// Terminal title set via OSC.
    title: Option<String>,
    /// Current working directory reported via OSC 7.
    cwd: Option<String>,
}

impl PaneScreen {
    /// Create a new screen with the given scrollback byte limit.
    #[must_use]
    pub fn new(max_scrollback_bytes: usize) -> Self {
        Self {
            parser: vte::Parser::new(),
            performer: ScreenPerformer {
                raw_bytes: Vec::new(),
                max_bytes: max_scrollback_bytes,
                cursor_row: 0,
                cursor_col: 0,
                title: None,
                cwd: None,
            },
        }
    }

    /// Feed raw PTY output bytes into the parser.
    pub fn feed(&mut self, data: &[u8]) {
        // Store raw bytes for snapshot replay.
        self.performer.raw_bytes.extend_from_slice(data);
        if self.performer.raw_bytes.len() > self.performer.max_bytes {
            let excess = self.performer.raw_bytes.len() - self.performer.max_bytes;
            self.performer.raw_bytes.drain(..excess);
        }

        for &byte in data {
            self.parser.advance(&mut self.performer, byte);
        }
    }

    /// Return the raw bytes for snapshot replay.
    #[must_use]
    pub fn raw_bytes(&self) -> &[u8] {
        &self.performer.raw_bytes
    }

    /// Return a tail slice of raw bytes suitable for client snapshot replay.
    ///
    /// Caps the returned data to `max_bytes` and finds a clean newline
    /// boundary so the snapshot never starts mid-escape-sequence.
    #[must_use]
    pub fn snapshot_bytes(&self, max_bytes: usize) -> &[u8] {
        let raw = &self.performer.raw_bytes;
        if raw.len() <= max_bytes {
            return raw;
        }
        let start = raw.len() - max_bytes;
        // Find the first newline at or after `start` to avoid splitting
        // mid-escape-sequence.
        raw[start..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(&raw[start..], |offset| &raw[start + offset + 1..])
    }

    /// Return the terminal title if set via OSC.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.performer.title.as_deref()
    }

    /// Return the current working directory if set via OSC 7.
    #[must_use]
    pub fn cwd(&self) -> Option<&str> {
        self.performer.cwd.as_deref()
    }

    /// Return cursor position (row, col).
    #[must_use]
    pub const fn cursor_position(&self) -> (usize, usize) {
        (self.performer.cursor_row, self.performer.cursor_col)
    }
}

/// Return restart-safe scrollback bytes for a reconstructed pane.
///
/// A daemon restart destroys the old PTY, so the last unterminated line from
/// the previous shell cannot be resumed safely. Keeping it would cause the new
/// shell prompt to be appended onto stale prompt/editing state, producing
/// duplicated prompts and corrupted command lines after reconstruction.
#[must_use]
pub fn restart_safe_scrollback(data: &[u8]) -> &[u8] {
    if data.last().is_some_and(|byte| matches!(byte, b'\n' | b'\r')) {
        return data;
    }

    match data.iter().rposition(|&byte| matches!(byte, b'\n' | b'\r')) {
        Some(index) => &data[..=index],
        None => &[],
    }
}

impl vte::Perform for ScreenPerformer {
    fn print(&mut self, c: char) {
        self.cursor_col += 1;
        let _ = c;
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                self.cursor_row += 1;
                self.cursor_col = 0;
            }
            b'\r' => {
                self.cursor_col = 0;
            }
            b'\x08' => {
                // Backspace
                self.cursor_col = self.cursor_col.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn hook(&mut self, _params: &vte::Params, _intermediates: &[u8], _ignore: bool, _action: char) {
    }

    fn put(&mut self, _byte: u8) {}

    fn unhook(&mut self) {}

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // OSC 0 and OSC 2 set the window title.
        if params.len() >= 2
            && (params[0] == b"0" || params[0] == b"2")
            && let Ok(title) = std::str::from_utf8(params[1])
        {
            self.title = Some(title.to_string());
        }

        if params.len() >= 2
            && params[0] == b"7"
            && let Ok(uri) = std::str::from_utf8(params[1])
            && let Some(cwd) = parse_osc7_current_directory(uri)
        {
            self.cwd = Some(cwd);
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        let first_param = params.iter().next().and_then(|p| p.first().copied()).unwrap_or(1);
        let n = first_param as usize;

        match action {
            // CUU — Cursor Up
            'A' => self.cursor_row = self.cursor_row.saturating_sub(n),
            // CUD — Cursor Down
            'B' => self.cursor_row += n,
            // CUF — Cursor Forward
            'C' => self.cursor_col += n,
            // CUB — Cursor Back
            'D' => self.cursor_col = self.cursor_col.saturating_sub(n),
            // CUP — Cursor Position
            'H' | 'f' => {
                let mut iter = params.iter();
                let row = iter.next().and_then(|p| p.first().copied()).unwrap_or(1);
                let col = iter.next().and_then(|p| p.first().copied()).unwrap_or(1);
                self.cursor_row = (row as usize).saturating_sub(1);
                self.cursor_col = (col as usize).saturating_sub(1);
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, _byte: u8) {}
}

fn parse_osc7_current_directory(uri: &str) -> Option<String> {
    let path_with_host = uri.strip_prefix("file://")?;
    let path_start = path_with_host.find('/')?;
    let encoded_path = &path_with_host[path_start..];
    percent_decode_path(encoded_path)
}

fn percent_decode_path(encoded: &str) -> Option<String> {
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hi = *bytes.get(index + 1)?;
            let lo = *bytes.get(index + 2)?;
            decoded.push((hex_value(hi)? << 4) | hex_value(lo)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8(decoded).ok()
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_echo_hello() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"hello");
        assert_eq!(screen.cursor_position(), (0, 5));
    }

    #[test]
    fn feed_newline_moves_cursor() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"hello\r\nworld");
        assert_eq!(screen.cursor_position(), (1, 5));
    }

    #[test]
    fn raw_bytes_preserved() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"test data");
        assert_eq!(screen.raw_bytes(), b"test data");
    }

    #[test]
    fn raw_bytes_capped_at_max() {
        let mut screen = PaneScreen::new(10);
        screen.feed(b"0123456789abcdef");
        assert_eq!(screen.raw_bytes().len(), 10);
        // Should keep the tail.
        assert_eq!(screen.raw_bytes(), b"6789abcdef");
    }

    #[test]
    fn osc_title_parsed() {
        let mut screen = PaneScreen::new(1024);
        // OSC 0 ; title BEL
        screen.feed(b"\x1b]0;my title\x07");
        assert_eq!(screen.title(), Some("my title"));
    }

    #[test]
    fn osc7_current_directory_is_parsed_and_percent_decoded() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b]7;file://localhost/tmp/work%20tree\x07");
        assert_eq!(screen.cwd(), Some("/tmp/work tree"));
    }

    #[test]
    fn cursor_up_down() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\r\n\r\n\r\n"); // row 3
        screen.feed(b"\x1b[2A"); // up 2 → row 1
        assert_eq!(screen.cursor_position().0, 1);
    }

    #[test]
    fn snapshot_bytes_returns_all_when_under_cap() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"line 1\nline 2\n");
        assert_eq!(screen.snapshot_bytes(1024), b"line 1\nline 2\n");
    }

    #[test]
    fn snapshot_bytes_caps_at_newline_boundary() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"aaaa\nbbbb\ncccc\n");
        // Cap to 10 bytes — start=5 ("bbbb\ncccc\n"), finds newline at
        // offset 4, returns from "cccc\n".
        let snap = screen.snapshot_bytes(10);
        assert_eq!(snap, b"cccc\n");
    }

    #[test]
    fn snapshot_bytes_avoids_mid_escape_sequence_split() {
        let mut screen = PaneScreen::new(1024);
        // ESC[31m (red) followed by text and newline, then more content
        screen.feed(b"old line\n\x1b[31mred text\nmore\n");
        // Cap to 15: start=15, raw[15..] = "red text\nmore\n"
        // finds newline at offset 8, returns "more\n"
        // Cap to 22: start=8, raw[8..] = "\n\x1b[31mred text\nmore\n"
        // finds newline at offset 0, returns "\x1b[31mred text\nmore\n"
        let snap = screen.snapshot_bytes(22);
        assert_eq!(snap, b"\x1b[31mred text\nmore\n");
    }

    #[test]
    fn snapshot_bytes_falls_back_to_raw_tail_when_no_newline() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"no newlines at all in this data");
        let snap = screen.snapshot_bytes(10);
        assert_eq!(snap, b" this data");
    }

    #[test]
    fn snapshot_bytes_large_buffer_stays_within_cap() {
        let mut screen = PaneScreen::new(1024 * 1024);
        // Simulate a long session with many lines of colored output.
        for i in 0..10_000 {
            screen.feed(format!("\x1b[32mline {i}: some output text\x1b[0m\n").as_bytes());
        }
        let cap = 4096;
        let snap = screen.snapshot_bytes(cap);
        assert!(snap.len() <= cap);
        // Should start at a line boundary, not mid-escape-sequence.
        assert!(
            snap[0] == b'\x1b' || snap[0].is_ascii_graphic() || snap[0].is_ascii_whitespace(),
            "snapshot starts with unexpected byte: 0x{:02x}",
            snap[0]
        );
    }

    #[test]
    fn restart_safe_scrollback_keeps_complete_lines() {
        assert_eq!(restart_safe_scrollback(b"line 1\r\nline 2\r\n"), b"line 1\r\nline 2\r\n");
    }

    #[test]
    fn restart_safe_scrollback_drops_unterminated_active_line() {
        assert_eq!(restart_safe_scrollback(b"line 1\r\nPROMPT> "), b"line 1\r\n");
    }

    #[test]
    fn restart_safe_scrollback_drops_prompt_only_state() {
        assert_eq!(restart_safe_scrollback(b"PROMPT> "), b"");
    }

    #[test]
    fn restart_safe_scrollback_uses_last_carriage_return_as_boundary() {
        assert_eq!(restart_safe_scrollback(b"status\rpartial"), b"status\r");
    }
}
