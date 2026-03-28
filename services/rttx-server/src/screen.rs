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

    /// Return the terminal title if set via OSC.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.performer.title.as_deref()
    }

    /// Return cursor position (row, col).
    #[must_use]
    pub const fn cursor_position(&self) -> (usize, usize) {
        (self.performer.cursor_row, self.performer.cursor_col)
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
    fn cursor_up_down() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\r\n\r\n\r\n"); // row 3
        screen.feed(b"\x1b[2A"); // up 2 → row 1
        assert_eq!(screen.cursor_position().0, 1);
    }
}
