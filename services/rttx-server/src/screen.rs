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
    /// Pending replies to write back to the PTY (e.g. CPR for DSR).
    pending_replies: Vec<Vec<u8>>,
    /// Whether bracketed paste mode (DECSET 2004) is active.
    bracketed_paste_mode: bool,
    /// Whether application cursor keys mode (DECSET 1) is active.
    application_cursor_keys: bool,
    /// Whether application keypad mode (DECKPAM) is active.
    application_keypad: bool,
    /// Active mouse tracking mode: 0=off, 1000/1002/1003.
    mouse_tracking_mode: u16,
    /// Whether SGR mouse mode (DECSET 1006) is active.
    sgr_mouse_mode: bool,
    /// Whether focus event mode (DECSET 1004) is active.
    focus_event_mode: bool,
    /// Whether the cursor is visible (DECSET 25, default true).
    cursor_visible: bool,
    /// Whether alternate screen buffer (DECSET 1049/1047) is active.
    alternate_screen: bool,
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
                pending_replies: Vec::new(),
                bracketed_paste_mode: false,
                application_cursor_keys: false,
                application_keypad: false,
                mouse_tracking_mode: 0,
                sgr_mouse_mode: false,
                focus_event_mode: false,
                cursor_visible: true,
                alternate_screen: false,
            },
        }
    }

    /// Feed raw PTY output bytes into the parser.
    pub fn feed(&mut self, data: &[u8]) {
        self.accept_raw(data);
        self.parse(data);
    }

    /// Store raw bytes for snapshot replay without running the VTE parser.
    ///
    /// This is the cheap half of [`feed`]: a memcpy into the scrollback
    /// buffer, safe to call while holding a contended lock. Call [`parse`]
    /// separately (potentially after releasing the lock) to run the
    /// expensive byte-by-byte VTE state machine.
    pub fn accept_raw(&mut self, data: &[u8]) {
        self.performer.raw_bytes.extend_from_slice(data);
        if self.performer.raw_bytes.len() > self.performer.max_bytes {
            let excess = self.performer.raw_bytes.len() - self.performer.max_bytes;
            self.performer.raw_bytes.drain(..excess);
            self.performer.raw_bytes.shrink_to(self.performer.max_bytes);
        }
    }

    /// Run the VTE parser over `data`, updating cursor, modes, title, and CWD.
    ///
    /// This is the expensive half of [`feed`]: it advances the VTE state
    /// machine byte-by-byte. Callers that need to minimize lock hold time
    /// should call [`accept_raw`] under the lock and defer [`parse`] until
    /// after releasing it.
    pub fn parse(&mut self, data: &[u8]) {
        for &byte in data {
            self.parser.advance(&mut self.performer, byte);
        }
    }

    /// Return the raw bytes for snapshot replay.
    #[must_use]
    pub fn raw_bytes(&self) -> &[u8] {
        &self.performer.raw_bytes
    }

    /// Return the allocated capacity of the raw bytes buffer.
    #[cfg(test)]
    #[must_use]
    pub const fn raw_bytes_capacity(&self) -> usize {
        self.performer.raw_bytes.capacity()
    }

    /// Release the in-memory scrollback buffer, freeing its allocation.
    pub fn clear_scrollback(&mut self) {
        self.performer.raw_bytes = Vec::new();
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

    /// Drain and return any pending replies (e.g. CPR for DSR).
    pub fn take_pending_replies(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.performer.pending_replies)
    }

    /// Whether bracketed paste mode (DECSET 2004) is currently active.
    #[must_use]
    pub const fn bracketed_paste_mode(&self) -> bool {
        self.performer.bracketed_paste_mode
    }

    /// Whether application cursor keys mode (DECSET 1) is currently active.
    #[must_use]
    pub const fn application_cursor_keys(&self) -> bool {
        self.performer.application_cursor_keys
    }

    /// Whether application keypad mode (DECKPAM) is currently active.
    #[must_use]
    pub const fn application_keypad(&self) -> bool {
        self.performer.application_keypad
    }

    /// Active mouse tracking mode: 0=off, 1000/1002/1003.
    #[must_use]
    pub const fn mouse_tracking_mode(&self) -> u16 {
        self.performer.mouse_tracking_mode
    }

    /// Whether SGR mouse mode (DECSET 1006) is currently active.
    #[must_use]
    pub const fn sgr_mouse_mode(&self) -> bool {
        self.performer.sgr_mouse_mode
    }

    /// Whether focus event mode (DECSET 1004) is currently active.
    #[must_use]
    pub const fn focus_event_mode(&self) -> bool {
        self.performer.focus_event_mode
    }

    /// Whether the cursor is visible (DECSET 25, default true).
    #[must_use]
    pub const fn cursor_visible(&self) -> bool {
        self.performer.cursor_visible
    }

    /// Whether alternate screen buffer (DECSET 1049/1047) is active.
    #[must_use]
    pub const fn alternate_screen(&self) -> bool {
        self.performer.alternate_screen
    }

    /// Build a consolidated `TerminalModeState` from the current screen state.
    #[must_use]
    pub fn terminal_mode_state(&self) -> rttx_proto::v3::TerminalModeState {
        rttx_proto::v3::TerminalModeState {
            bracketed_paste: self.performer.bracketed_paste_mode,
            focus_reporting: self.performer.focus_event_mode,
            application_cursor_keys: self.performer.application_cursor_keys,
            application_keypad: self.performer.application_keypad,
            alternate_screen: self.performer.alternate_screen,
            cursor_hidden: !self.performer.cursor_visible,
            mouse_mode: rttx_proto::v3_terminal_modes::mouse_mode_from_tracking_value(
                self.performer.mouse_tracking_mode,
            ) as i32,
            sgr_mouse: self.performer.sgr_mouse_mode,
        }
    }

    /// Restore terminal mode flags from a persisted snapshot.
    ///
    /// Called during daemon restart reconstruction so that canonical mode
    /// state comes from the snapshot metadata rather than depending on the
    /// mode-setting escape sequences being present in `screen_bytes`.
    pub const fn restore_modes(&mut self, modes: &crate::state::types::TerminalModeSnapshot) {
        self.performer.bracketed_paste_mode = modes.bracketed_paste;
        self.performer.application_cursor_keys = modes.application_cursor_keys;
        self.performer.application_keypad = modes.application_keypad;
        self.performer.mouse_tracking_mode = modes.mouse_tracking_mode;
        self.performer.sgr_mouse_mode = modes.sgr_mouse;
        self.performer.focus_event_mode = modes.focus_reporting;
    }

    /// Restore cursor visibility from a persisted snapshot.
    pub const fn restore_cursor_visible(&mut self, visible: bool) {
        self.performer.cursor_visible = visible;
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
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        let first_param = params.iter().next().and_then(|p| p.first().copied()).unwrap_or(1);
        let n = first_param as usize;

        // DECSET/DECRST — private mode set/reset (CSI ? Pm h/l)
        if intermediates == b"?" && matches!(action, 'h' | 'l') {
            let enabled = action == 'h';
            for param_slice in params {
                match param_slice.first().copied() {
                    Some(1) => self.application_cursor_keys = enabled,
                    Some(25) => self.cursor_visible = enabled,
                    Some(1000) => {
                        self.mouse_tracking_mode = if enabled { 1000 } else { 0 };
                    }
                    Some(1002) => {
                        self.mouse_tracking_mode = if enabled { 1002 } else { 0 };
                    }
                    Some(1003) => {
                        self.mouse_tracking_mode = if enabled { 1003 } else { 0 };
                    }
                    Some(1004) => self.focus_event_mode = enabled,
                    Some(1006) => self.sgr_mouse_mode = enabled,
                    Some(1047 | 1049) => self.alternate_screen = enabled,
                    Some(2004) => self.bracketed_paste_mode = enabled,
                    _ => {}
                }
            }
            return;
        }

        // DECRQM — Request Mode (CSI ? Ps $ p) → DECRPM (CSI ? Ps ; Pm $ y)
        // Pm: 0=not recognized, 1=set, 2=reset
        if intermediates == b"?$" && action == 'p' {
            let mode = first_param;
            let is_set = match mode {
                1 => Some(self.application_cursor_keys),
                25 => Some(self.cursor_visible),
                1000 | 1002 | 1003 => Some(self.mouse_tracking_mode == mode),
                1004 => Some(self.focus_event_mode),
                1006 => Some(self.sgr_mouse_mode),
                1047 | 1049 => Some(self.alternate_screen),
                2004 => Some(self.bracketed_paste_mode),
                _ => None,
            };
            let pm = match is_set {
                Some(true) => 1,
                Some(false) => 2,
                None => 0,
            };
            let reply = format!("\x1b[?{mode};{pm}$y");
            self.pending_replies.push(reply.into_bytes());
            return;
        }

        // DA2 — Secondary Device Attributes (CSI > c)
        if intermediates == b">" && action == 'c' {
            self.pending_replies.push(b"\x1b[>65;0;0c".to_vec());
            return;
        }

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
            // DSR — Device Status Report
            'n' => match first_param {
                5 => {
                    // Operating status: report "OK"
                    self.pending_replies.push(b"\x1b[0n".to_vec());
                }
                6 => {
                    // Cursor position: report CPR (1-based)
                    let reply = format!("\x1b[{};{}R", self.cursor_row + 1, self.cursor_col + 1);
                    self.pending_replies.push(reply.into_bytes());
                }
                _ => {}
            },
            // DA1 — Primary Device Attributes (CSI c / CSI 0 c)
            'c' if intermediates.is_empty() && first_param <= 1 => {
                // VT420 with 132-column, printer, selective erase, ANSI color
                self.pending_replies.push(b"\x1b[?64;1;2;6;22c".to_vec());
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            // DECKPAM — Application Keypad Mode
            b'=' => self.application_keypad = true,
            // DECKPNM — Normal Keypad Mode
            b'>' => self.application_keypad = false,
            _ => {}
        }
    }
}

/// Strip terminal query sequences that the daemon handles server-side.
///
/// Applications send DSR, DA1, DA2, and DECRQM queries expecting the terminal
/// to respond. The daemon's `PaneScreen` already intercepts these and writes
/// replies back to the PTY. If the raw queries are also forwarded to the
/// client's VTE widget, VTE generates duplicate responses that leak back as
/// visible garbage (`;1R` fragments).
///
/// This function removes those query sequences from the output stream before
/// it is broadcast to clients. Non-query bytes pass through unchanged.
///
/// Stripped sequences:
/// - `ESC [ 5 n` / `ESC [ 6 n` (DSR)
/// - `ESC [ c` / `ESC [ 0 c` (DA1)
/// - `ESC [ > c` / `ESC [ > 0 c` (DA2)
/// - `ESC [ ? <digits> $ p` (DECRQM)
#[must_use]
pub fn strip_client_queries(data: &[u8]) -> Vec<u8> {
    // Fast path: no ESC byte means no CSI sequences to strip.
    if !data.contains(&0x1b) {
        return data.to_vec();
    }

    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;

    while i < data.len() {
        if data[i] == 0x1b
            && let Some(seq_len) = csi_query_len(&data[i..])
        {
            i += seq_len;
        } else {
            out.push(data[i]);
            i += 1;
        }
    }

    out
}

/// If `data` starts with a CSI query sequence handled by the daemon, return
/// its length in bytes. Otherwise return `None`.
fn csi_query_len(data: &[u8]) -> Option<usize> {
    // Minimum CSI sequence: ESC [ <final> = 3 bytes.
    if data.len() < 3 || data[0] != 0x1b || data[1] != b'[' {
        return None;
    }

    let mut pos = 2;

    // Collect intermediates (0x20..=0x2F) and parameter bytes (0x30..=0x3F).
    // CSI structure: ESC [ <param bytes 0x30-0x3F>* <intermediate bytes 0x20-0x2F>* <final 0x40-0x7E>
    let param_start = pos;
    while pos < data.len() && (0x30..=0x3F).contains(&data[pos]) {
        pos += 1;
    }
    let params = &data[param_start..pos];

    let inter_start = pos;
    while pos < data.len() && (0x20..=0x2F).contains(&data[pos]) {
        pos += 1;
    }
    let intermediates = &data[inter_start..pos];

    // Final byte must be in 0x40..=0x7E.
    if pos >= data.len() || !(0x40..=0x7E).contains(&data[pos]) {
        return None;
    }
    let final_byte = data[pos];
    let seq_len = pos + 1;

    match final_byte {
        // DSR: CSI 5 n, CSI 6 n, CSI ? 5 n, CSI ? 6 n
        b'n' if intermediates.is_empty() && matches!(params, b"5" | b"6" | b"?5" | b"?6") => {
            Some(seq_len)
        }
        // CPR (Cursor Position Report): CSI <digits> ; <digits> R
        // These are responses to DSR queries — never useful as display output.
        b'R' if intermediates.is_empty()
            && !params.is_empty()
            && params.iter().all(|&b| b.is_ascii_digit() || b == b';') =>
        {
            Some(seq_len)
        }
        // DA1: CSI c, CSI 0 c
        b'c' if intermediates.is_empty() && matches!(params, b"" | b"0") => Some(seq_len),
        // DA2: ESC [ > c or ESC [ > 0 c
        // `>` (0x3E) falls in the param byte range, so the parser puts it in params.
        b'c' if intermediates.is_empty() && matches!(params, b">" | b">0") => Some(seq_len),
        // DECRQM: ESC [ ? <digits> $ p
        // `?` (0x3F) is a param byte, `$` (0x24) is an intermediate byte.
        b'p' if intermediates == b"$"
            && params.first() == Some(&b'?')
            && params.len() >= 2
            && params[1..].iter().all(u8::is_ascii_digit) =>
        {
            Some(seq_len)
        }
        _ => None,
    }
}

/// Terminal cleanup byte sequence fed into a pane's screen when its
/// process exits.
///
/// Resets every mode a TUI might have left enabled so that reconnecting
/// clients and persisted snapshots see a clean terminal state.
///
/// Contents (in order):
/// 1. `CAN` (`\x18`) — abort any in-progress escape sequence
/// 2. Exit alt-screen: DECRST 1049 (modern) + DECRST 47 (legacy)
/// 3. Show cursor: DECSET 25
/// 4. Disable mouse: DECRST 1000, 1002, 1003, 1006, 1015
/// 5. Disable focus reporting: DECRST 1004
/// 6. Normal cursor keys: DECRST 1
/// 7. Numeric keypad: DECPNM (`ESC >`)
/// 8. Disable bracketed paste: DECRST 2004
/// 9. Reset SGR: `ESC [ m`
#[must_use]
pub const fn terminal_cleanup_bytes() -> &'static [u8] {
    b"\x18\
      \x1b[?1049l\x1b[?47l\
      \x1b[?25h\
      \x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1015l\
      \x1b[?1004l\
      \x1b[?1l\x1b>\
      \x1b[?2004l\
      \x1b[m"
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
    fn clear_scrollback_releases_buffer() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"hello world");
        assert!(!screen.raw_bytes().is_empty());

        screen.clear_scrollback();
        assert!(screen.raw_bytes().is_empty());
        // Capacity should be released, not just length zeroed.
        assert_eq!(screen.raw_bytes().len(), 0);
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
    fn raw_bytes_capacity_shrinks_after_drain() {
        let max = 1024;
        let mut screen = PaneScreen::new(max);
        // Feed a large burst that exceeds max, triggering drain.
        let burst = vec![b'X'; max * 3];
        screen.feed(&burst);
        assert_eq!(screen.raw_bytes().len(), max);
        // Capacity should be close to max, not 3x max.
        let cap = screen.raw_bytes_capacity();
        assert!(cap <= max * 2, "capacity {cap} should shrink toward max {max} after drain");
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
    fn dsr_cursor_position_generates_cpr_response() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"hello"); // cursor at (0, 5)
        screen.feed(b"\x1b[6n"); // DSR: request cursor position
        let responses = screen.take_pending_replies();
        assert_eq!(responses.len(), 1);
        // CPR is 1-based: row=1, col=6
        assert_eq!(responses[0], b"\x1b[1;6R");
    }

    #[test]
    fn dsr_after_cursor_movement_reports_correct_position() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"line1\r\nline2\r\nline3");
        screen.feed(b"\x1b[6n");
        let responses = screen.take_pending_replies();
        assert_eq!(responses.len(), 1);
        // row=3, col=6 (1-based)
        assert_eq!(responses[0], b"\x1b[3;6R");
    }

    #[test]
    fn dsr_operating_status_generates_ok_response() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[5n"); // DSR: request operating status
        let responses = screen.take_pending_replies();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0], b"\x1b[0n"); // "OK" status
    }

    #[test]
    fn no_pending_replies_without_dsr() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"hello world");
        assert!(screen.take_pending_replies().is_empty());
    }

    #[test]
    fn multiple_dsr_in_single_feed() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"ab\x1b[6n\x1b[6n");
        let responses = screen.take_pending_replies();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0], b"\x1b[1;3R");
        assert_eq!(responses[1], b"\x1b[1;3R");
    }

    #[test]
    fn take_pending_replies_drains() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[6n");
        assert_eq!(screen.take_pending_replies().len(), 1);
        assert!(screen.take_pending_replies().is_empty());
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

    #[test]
    fn bracketed_paste_mode_default_off() {
        let screen = PaneScreen::new(1024);
        assert!(!screen.bracketed_paste_mode());
    }

    #[test]
    fn decset_2004_enables_bracketed_paste_mode() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?2004h");
        assert!(screen.bracketed_paste_mode());
    }

    #[test]
    fn decrst_2004_disables_bracketed_paste_mode() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?2004h");
        assert!(screen.bracketed_paste_mode());
        screen.feed(b"\x1b[?2004l");
        assert!(!screen.bracketed_paste_mode());
    }

    #[test]
    fn bracketed_paste_mode_survives_interleaved_output() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?2004h");
        screen.feed(b"some terminal output\r\n");
        assert!(screen.bracketed_paste_mode());
    }

    #[test]
    fn other_private_modes_do_not_affect_bracketed_paste() {
        let mut screen = PaneScreen::new(1024);
        // DECSET 1049 (alternate screen) should not enable bracketed paste
        screen.feed(b"\x1b[?1049h");
        assert!(!screen.bracketed_paste_mode());
    }

    #[test]
    fn application_cursor_keys_default_off() {
        let screen = PaneScreen::new(1024);
        assert!(!screen.application_cursor_keys());
    }

    #[test]
    fn decset_1_enables_application_cursor_keys() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?1h");
        assert!(screen.application_cursor_keys());
    }

    #[test]
    fn decrst_1_disables_application_cursor_keys() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?1h");
        screen.feed(b"\x1b[?1l");
        assert!(!screen.application_cursor_keys());
    }

    #[test]
    fn application_keypad_default_off() {
        let screen = PaneScreen::new(1024);
        assert!(!screen.application_keypad());
    }

    #[test]
    fn deckpam_enables_application_keypad() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b=");
        assert!(screen.application_keypad());
    }

    #[test]
    fn deckpnm_disables_application_keypad() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b=");
        screen.feed(b"\x1b>");
        assert!(!screen.application_keypad());
    }

    #[test]
    fn mouse_tracking_default_none() {
        let screen = PaneScreen::new(1024);
        assert_eq!(screen.mouse_tracking_mode(), 0);
    }

    #[test]
    fn decset_1000_enables_basic_mouse_tracking() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?1000h");
        assert_eq!(screen.mouse_tracking_mode(), 1000);
    }

    #[test]
    fn decset_1002_enables_button_event_tracking() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?1002h");
        assert_eq!(screen.mouse_tracking_mode(), 1002);
    }

    #[test]
    fn decset_1003_enables_any_event_tracking() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?1003h");
        assert_eq!(screen.mouse_tracking_mode(), 1003);
    }

    #[test]
    fn decrst_1003_disables_mouse_tracking() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?1003h");
        screen.feed(b"\x1b[?1003l");
        assert_eq!(screen.mouse_tracking_mode(), 0);
    }

    #[test]
    fn sgr_mouse_mode_default_off() {
        let screen = PaneScreen::new(1024);
        assert!(!screen.sgr_mouse_mode());
    }

    #[test]
    fn decset_1006_enables_sgr_mouse_mode() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?1006h");
        assert!(screen.sgr_mouse_mode());
    }

    #[test]
    fn decrst_1006_disables_sgr_mouse_mode() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?1006h");
        screen.feed(b"\x1b[?1006l");
        assert!(!screen.sgr_mouse_mode());
    }

    #[test]
    fn multiple_modes_tracked_independently() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?1h\x1b[?1003h\x1b[?1006h\x1b[?2004h");
        assert!(screen.application_cursor_keys());
        assert_eq!(screen.mouse_tracking_mode(), 1003);
        assert!(screen.sgr_mouse_mode());
        assert!(screen.bracketed_paste_mode());

        screen.feed(b"\x1b[?1l");
        assert!(!screen.application_cursor_keys());
        assert_eq!(screen.mouse_tracking_mode(), 1003);
    }

    // --- DA1 (Primary Device Attributes) ---

    #[test]
    fn da1_generates_response() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[c");
        let replies = screen.take_pending_replies();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0], b"\x1b[?64;1;2;6;22c");
    }

    #[test]
    fn da1_explicit_zero_param_generates_response() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[0c");
        let replies = screen.take_pending_replies();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0], b"\x1b[?64;1;2;6;22c");
    }

    #[test]
    fn da1_nonzero_param_ignored() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[2c");
        assert!(screen.take_pending_replies().is_empty());
    }

    // --- DA2 (Secondary Device Attributes) ---

    #[test]
    fn da2_generates_response() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[>c");
        let replies = screen.take_pending_replies();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0], b"\x1b[>65;0;0c");
    }

    #[test]
    fn da2_explicit_zero_param_generates_response() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[>0c");
        let replies = screen.take_pending_replies();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0], b"\x1b[>65;0;0c");
    }

    // --- Focus event mode (DECSET 1004) ---

    #[test]
    fn focus_event_mode_default_off() {
        let screen = PaneScreen::new(1024);
        assert!(!screen.focus_event_mode());
    }

    #[test]
    fn decset_1004_enables_focus_event_mode() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?1004h");
        assert!(screen.focus_event_mode());
    }

    #[test]
    fn decrst_1004_disables_focus_event_mode() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?1004h");
        screen.feed(b"\x1b[?1004l");
        assert!(!screen.focus_event_mode());
    }

    // --- Cursor visibility (DECSET 25) ---

    #[test]
    fn cursor_visible_default_on() {
        let screen = PaneScreen::new(1024);
        assert!(screen.cursor_visible());
    }

    #[test]
    fn decrst_25_hides_cursor() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?25l");
        assert!(!screen.cursor_visible());
    }

    #[test]
    fn decset_25_shows_cursor() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?25l");
        screen.feed(b"\x1b[?25h");
        assert!(screen.cursor_visible());
    }

    // --- Alternate screen (DECSET 1049/1047) ---

    #[test]
    fn alternate_screen_default_off() {
        let screen = PaneScreen::new(1024);
        assert!(!screen.alternate_screen());
    }

    #[test]
    fn decset_1049_enables_alternate_screen() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?1049h");
        assert!(screen.alternate_screen());
    }

    #[test]
    fn decrst_1049_disables_alternate_screen() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?1049h");
        screen.feed(b"\x1b[?1049l");
        assert!(!screen.alternate_screen());
    }

    #[test]
    fn decset_1047_enables_alternate_screen() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?1047h");
        assert!(screen.alternate_screen());
    }

    #[test]
    fn decrst_1047_disables_alternate_screen() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?1047h");
        screen.feed(b"\x1b[?1047l");
        assert!(!screen.alternate_screen());
    }

    // --- terminal_mode_state() consolidation ---

    #[test]
    fn terminal_mode_state_defaults() {
        let screen = PaneScreen::new(1024);
        let state = screen.terminal_mode_state();
        assert_eq!(state, rttx_proto::v3::TerminalModeState::default());
    }

    #[test]
    fn terminal_mode_state_reflects_all_modes() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?2004h"); // bracketed paste
        screen.feed(b"\x1b[?1004h"); // focus reporting
        screen.feed(b"\x1b[?1h"); // application cursor keys
        screen.feed(b"\x1b="); // application keypad
        screen.feed(b"\x1b[?1049h"); // alternate screen
        screen.feed(b"\x1b[?25l"); // hide cursor
        screen.feed(b"\x1b[?1003h"); // any-event mouse
        screen.feed(b"\x1b[?1006h"); // SGR mouse

        let state = screen.terminal_mode_state();
        assert!(state.bracketed_paste);
        assert!(state.focus_reporting);
        assert!(state.application_cursor_keys);
        assert!(state.application_keypad);
        assert!(state.alternate_screen);
        assert!(state.cursor_hidden);
        assert_eq!(state.mouse_mode, rttx_proto::v3::MouseMode::Any as i32);
        assert!(state.sgr_mouse);
    }

    #[test]
    fn terminal_mode_state_cursor_hidden_inverts_cursor_visible() {
        let mut screen = PaneScreen::new(1024);
        assert!(!screen.terminal_mode_state().cursor_hidden);
        screen.feed(b"\x1b[?25l");
        assert!(screen.terminal_mode_state().cursor_hidden);
        screen.feed(b"\x1b[?25h");
        assert!(!screen.terminal_mode_state().cursor_hidden);
    }

    // --- DECRQM (Request Mode) ---

    #[test]
    fn decrqm_reports_bracketed_paste_set() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?2004h");
        screen.feed(b"\x1b[?2004$p");
        let replies = screen.take_pending_replies();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0], b"\x1b[?2004;1$y");
    }

    #[test]
    fn decrqm_reports_bracketed_paste_reset() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?2004$p");
        let replies = screen.take_pending_replies();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0], b"\x1b[?2004;2$y");
    }

    #[test]
    fn decrqm_reports_unknown_mode() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?9999$p");
        let replies = screen.take_pending_replies();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0], b"\x1b[?9999;0$y");
    }

    #[test]
    fn decrqm_reports_application_cursor_keys() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?1h");
        screen.feed(b"\x1b[?1$p");
        let replies = screen.take_pending_replies();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0], b"\x1b[?1;1$y");
    }

    #[test]
    fn decrqm_reports_cursor_visible() {
        let mut screen = PaneScreen::new(1024);
        // Default is visible (set)
        screen.feed(b"\x1b[?25$p");
        let replies = screen.take_pending_replies();
        assert_eq!(replies[0], b"\x1b[?25;1$y");
    }

    #[test]
    fn decrqm_reports_focus_event_mode() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?1004h");
        screen.feed(b"\x1b[?1004$p");
        let replies = screen.take_pending_replies();
        assert_eq!(replies[0], b"\x1b[?1004;1$y");
    }

    #[test]
    fn decrqm_reports_mouse_tracking_mode() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?1003h");
        // Query 1003 — should be set
        screen.feed(b"\x1b[?1003$p");
        let replies = screen.take_pending_replies();
        assert_eq!(replies[0], b"\x1b[?1003;1$y");
        // Query 1000 — should be reset (1003 is active, not 1000)
        screen.feed(b"\x1b[?1000$p");
        let replies = screen.take_pending_replies();
        assert_eq!(replies[0], b"\x1b[?1000;2$y");
    }

    #[test]
    fn decrqm_reports_alternate_screen() {
        let mut screen = PaneScreen::new(1024);
        screen.feed(b"\x1b[?1049h");
        screen.feed(b"\x1b[?1049$p");
        let replies = screen.take_pending_replies();
        assert_eq!(replies[0], b"\x1b[?1049;1$y");
    }

    // --- Detached session scenario ---

    #[test]
    fn da1_and_dsr_work_without_client_attachment() {
        let mut screen = PaneScreen::new(1024);
        // Simulate application startup sequence without any client
        screen.feed(b"\x1b[c"); // DA1
        screen.feed(b"\x1b[>c"); // DA2
        screen.feed(b"\x1b[5n"); // DSR status
        screen.feed(b"\x1b[6n"); // DSR cursor
        let replies = screen.take_pending_replies();
        assert_eq!(replies.len(), 4);
        assert_eq!(replies[0], b"\x1b[?64;1;2;6;22c"); // DA1
        assert_eq!(replies[1], b"\x1b[>65;0;0c"); // DA2
        assert_eq!(replies[2], b"\x1b[0n"); // status OK
        assert_eq!(replies[3], b"\x1b[1;1R"); // cursor at 1,1
    }

    // --- strip_client_queries ---

    #[test]
    fn strip_passes_plain_text_through() {
        assert_eq!(strip_client_queries(b"hello world"), b"hello world");
    }

    #[test]
    fn strip_passes_empty_input_through() {
        assert_eq!(strip_client_queries(b""), b"");
    }

    #[test]
    fn strip_removes_dsr_cursor_position() {
        assert_eq!(strip_client_queries(b"before\x1b[6nafter"), b"beforeafter");
    }

    #[test]
    fn strip_removes_dsr_operating_status() {
        assert_eq!(strip_client_queries(b"before\x1b[5nafter"), b"beforeafter");
    }

    #[test]
    fn strip_removes_cpr_response() {
        // CSI 63;1 R — cursor position report (row 63, col 1)
        assert_eq!(strip_client_queries(b"before\x1b[63;1Rafter"), b"beforeafter");
        // Multiple CPR responses in sequence
        assert_eq!(strip_client_queries(b"\x1b[63;1R\x1b[63;2R\x1b[30;1R"), b"");
    }

    #[test]
    fn strip_removes_decxcpr_query() {
        // CSI ? 6 n — extended cursor position query
        assert_eq!(strip_client_queries(b"before\x1b[?6nafter"), b"beforeafter");
    }

    #[test]
    fn strip_removes_da1_no_param() {
        assert_eq!(strip_client_queries(b"before\x1b[cafter"), b"beforeafter");
    }

    #[test]
    fn strip_removes_da1_zero_param() {
        assert_eq!(strip_client_queries(b"before\x1b[0cafter"), b"beforeafter");
    }

    #[test]
    fn strip_removes_da2() {
        assert_eq!(strip_client_queries(b"before\x1b[>cafter"), b"beforeafter");
    }

    #[test]
    fn strip_removes_da2_zero_param() {
        assert_eq!(strip_client_queries(b"before\x1b[>0cafter"), b"beforeafter");
    }

    #[test]
    fn strip_removes_decrqm() {
        assert_eq!(strip_client_queries(b"before\x1b[?2004$pafter"), b"beforeafter");
    }

    #[test]
    fn strip_removes_multiple_queries() {
        let input = b"text\x1b[6n\x1b[c\x1b[>cmore\x1b[5n";
        assert_eq!(strip_client_queries(input), b"textmore");
    }

    #[test]
    fn strip_preserves_non_query_csi_sequences() {
        // SGR (color), CUP (cursor position), CUU (cursor up) should pass through.
        let input = b"\x1b[31mred\x1b[0m \x1b[1;1H \x1b[2A";
        assert_eq!(strip_client_queries(input), input.to_vec());
    }

    #[test]
    fn strip_preserves_osc_sequences() {
        let input = b"\x1b]0;title\x07text";
        assert_eq!(strip_client_queries(input), input.to_vec());
    }

    #[test]
    fn strip_preserves_decset_decrst() {
        // DECSET/DECRST should not be stripped.
        let input = b"\x1b[?2004h\x1b[?1l";
        assert_eq!(strip_client_queries(input), input.to_vec());
    }

    #[test]
    fn strip_handles_query_at_start() {
        assert_eq!(strip_client_queries(b"\x1b[6ntext"), b"text");
    }

    #[test]
    fn strip_handles_query_at_end() {
        assert_eq!(strip_client_queries(b"text\x1b[6n"), b"text");
    }

    #[test]
    fn strip_handles_only_queries() {
        assert_eq!(strip_client_queries(b"\x1b[6n\x1b[c\x1b[>c"), b"");
    }

    #[test]
    fn strip_handles_incomplete_csi_at_end() {
        // Incomplete CSI at end of buffer should pass through.
        let input = b"text\x1b[";
        assert_eq!(strip_client_queries(input), input.to_vec());
    }

    #[test]
    fn strip_handles_bare_esc_at_end() {
        let input = b"text\x1b";
        assert_eq!(strip_client_queries(input), input.to_vec());
    }

    #[test]
    fn strip_does_not_remove_da1_with_nonzero_param() {
        // CSI 2 c is not DA1 (only 0 or no param).
        let input = b"\x1b[2c";
        assert_eq!(strip_client_queries(input), input.to_vec());
    }

    #[test]
    fn strip_does_not_remove_dsr_with_other_params() {
        // CSI 1 n is not a handled DSR.
        let input = b"\x1b[1n";
        assert_eq!(strip_client_queries(input), input.to_vec());
    }

    #[test]
    fn strip_interleaved_with_real_output() {
        // Simulate rapid DSR queries mixed with real application output.
        let input = b"line1\r\n\x1b[6n\x1b[6nline2\r\n\x1b[6n\x1b[c";
        assert_eq!(strip_client_queries(input), b"line1\r\nline2\r\n");
    }

    // --- Scrollback replay: DSR queries must not generate stale replies ---

    #[test]
    fn replay_stripped_scrollback_produces_no_pending_replies() {
        // Simulate scrollback data that was stored with DSR queries.
        let scrollback = b"line1\r\n\x1b[6nline2\r\n\x1b[c\x1b[>c\x1b[5n";
        let cleaned = strip_client_queries(scrollback);

        let mut screen = PaneScreen::new(1024);
        screen.feed(&cleaned);

        // No stale replies should be generated from cleaned scrollback.
        assert!(screen.take_pending_replies().is_empty());
        // Real content should be preserved.
        assert_eq!(screen.raw_bytes(), b"line1\r\nline2\r\n");
    }

    #[test]
    fn raw_scrollback_replay_generates_stale_replies() {
        // Demonstrates the bug: replaying raw scrollback with DSR queries
        // generates stale pending_replies that would be written to the PTY.
        let scrollback = b"line1\r\n\x1b[6nline2\r\n";

        let mut screen = PaneScreen::new(1024);
        screen.feed(scrollback);

        let replies = screen.take_pending_replies();
        assert!(!replies.is_empty(), "raw replay should generate stale replies (the bug)");
    }

    // --- restore_modes ---

    #[test]
    fn restore_modes_sets_all_mode_flags() {
        use crate::state::types::TerminalModeSnapshot;

        let mut screen = PaneScreen::new(1024);
        assert!(!screen.bracketed_paste_mode());
        assert!(!screen.application_cursor_keys());
        assert!(!screen.application_keypad());
        assert_eq!(screen.mouse_tracking_mode(), 0);
        assert!(!screen.sgr_mouse_mode());
        assert!(!screen.focus_event_mode());

        screen.restore_modes(&TerminalModeSnapshot {
            bracketed_paste: true,
            application_cursor_keys: true,
            application_keypad: true,
            mouse_tracking_mode: 1003,
            sgr_mouse: true,
            focus_reporting: true,
        });

        assert!(screen.bracketed_paste_mode());
        assert!(screen.application_cursor_keys());
        assert!(screen.application_keypad());
        assert_eq!(screen.mouse_tracking_mode(), 1003);
        assert!(screen.sgr_mouse_mode());
        assert!(screen.focus_event_mode());
    }

    #[test]
    fn restore_modes_overrides_replay_derived_state() {
        use crate::state::types::TerminalModeSnapshot;

        let mut screen = PaneScreen::new(1024);
        // Replay sets bracketed paste on.
        screen.feed(b"\x1b[?2004h");
        assert!(screen.bracketed_paste_mode());

        // Snapshot says it was off (mode-disabling escape was outside retained bytes).
        screen.restore_modes(&TerminalModeSnapshot {
            bracketed_paste: false,
            application_cursor_keys: false,
            application_keypad: false,
            mouse_tracking_mode: 0,
            sgr_mouse: false,
            focus_reporting: false,
        });

        assert!(!screen.bracketed_paste_mode());
    }

    #[test]
    fn restore_cursor_visible_overrides_default() {
        let mut screen = PaneScreen::new(1024);
        assert!(screen.cursor_visible());

        screen.restore_cursor_visible(false);
        assert!(!screen.cursor_visible());
    }

    #[test]
    fn restore_cursor_visible_overrides_replay_derived_state() {
        let mut screen = PaneScreen::new(1024);
        // Replay hides cursor.
        screen.feed(b"\x1b[?25l");
        assert!(!screen.cursor_visible());

        // Snapshot says cursor was visible.
        screen.restore_cursor_visible(true);
        assert!(screen.cursor_visible());
    }

    // ── terminal_cleanup_bytes ──────────────────────────────────────

    #[test]
    fn terminal_cleanup_bytes_contains_required_sequences() {
        let bytes = terminal_cleanup_bytes();
        assert_eq!(bytes[0], 0x18, "must start with CAN");
        assert!(bytes.windows(8).any(|w| w == b"\x1b[?1049l"), "alt-screen modern off");
        assert!(bytes.windows(6).any(|w| w == b"\x1b[?47l"), "alt-screen legacy off");
        assert!(bytes.windows(6).any(|w| w == b"\x1b[?25h"), "cursor visible");
        assert!(bytes.windows(8).any(|w| w == b"\x1b[?1000l"), "mouse normal off");
        assert!(bytes.windows(8).any(|w| w == b"\x1b[?1002l"), "mouse button off");
        assert!(bytes.windows(8).any(|w| w == b"\x1b[?1003l"), "mouse any off");
        assert!(bytes.windows(8).any(|w| w == b"\x1b[?1006l"), "SGR mouse off");
        assert!(bytes.windows(8).any(|w| w == b"\x1b[?1015l"), "urxvt mouse off");
        assert!(bytes.windows(8).any(|w| w == b"\x1b[?1004l"), "focus reporting off");
        assert!(bytes.windows(5).any(|w| w == b"\x1b[?1l"), "DECCKM off");
        assert!(bytes.windows(2).any(|w| w == b"\x1b>"), "DECPNM");
        assert!(bytes.windows(8).any(|w| w == b"\x1b[?2004l"), "bracketed paste off");
        assert!(bytes.windows(3).any(|w| w == b"\x1b[m"), "SGR reset");
    }

    #[test]
    fn cleanup_bytes_reset_dirty_screen_state() {
        let mut screen = PaneScreen::new(4096);
        // Simulate a TUI that enabled everything.
        screen.feed(b"\x1b[?1049h"); // alt-screen
        screen.feed(b"\x1b[?25l"); // hide cursor
        screen.feed(b"\x1b[?1003h"); // any-event mouse
        screen.feed(b"\x1b[?1006h"); // SGR mouse
        screen.feed(b"\x1b[?1004h"); // focus reporting
        screen.feed(b"\x1b[?1h"); // application cursor keys
        screen.feed(b"\x1b="); // application keypad
        screen.feed(b"\x1b[?2004h"); // bracketed paste

        assert!(screen.alternate_screen());
        assert!(!screen.cursor_visible());
        assert_eq!(screen.mouse_tracking_mode(), 1003);
        assert!(screen.sgr_mouse_mode());
        assert!(screen.focus_event_mode());
        assert!(screen.application_cursor_keys());
        assert!(screen.application_keypad());
        assert!(screen.bracketed_paste_mode());

        // Feed cleanup.
        screen.feed(terminal_cleanup_bytes());

        assert!(!screen.alternate_screen());
        assert!(screen.cursor_visible());
        assert_eq!(screen.mouse_tracking_mode(), 0);
        assert!(!screen.sgr_mouse_mode());
        assert!(!screen.focus_event_mode());
        assert!(!screen.application_cursor_keys());
        assert!(!screen.application_keypad());
        assert!(!screen.bracketed_paste_mode());
    }

    // ── accept_raw / parse split tests ──────────────────────────────

    #[test]
    fn accept_raw_stores_bytes_without_parsing() {
        let mut screen = PaneScreen::new(1024);
        screen.accept_raw(b"\x1b]0;title\x07hello");
        assert_eq!(screen.raw_bytes(), b"\x1b]0;title\x07hello");
        // VTE not run — title not extracted.
        assert!(screen.title().is_none());
        assert_eq!(screen.cursor_position(), (0, 0));
    }

    #[test]
    fn parse_advances_vte_state_machine() {
        let mut screen = PaneScreen::new(1024);
        screen.accept_raw(b"\x1b]0;title\x07hello");
        screen.parse(b"\x1b]0;title\x07hello");
        assert_eq!(screen.title(), Some("title"));
        assert_eq!(screen.cursor_position(), (0, 5));
    }

    #[test]
    fn accept_raw_then_parse_equivalent_to_feed() {
        let data = b"\x1b[?2004h\x1b]7;file://localhost/tmp\x07text\r\n\x1b[6n";

        let mut screen_a = PaneScreen::new(1024);
        screen_a.feed(data);

        let mut screen_b = PaneScreen::new(1024);
        screen_b.accept_raw(data);
        screen_b.parse(data);

        assert_eq!(screen_a.raw_bytes(), screen_b.raw_bytes());
        assert_eq!(screen_a.cursor_position(), screen_b.cursor_position());
        assert_eq!(screen_a.cwd(), screen_b.cwd());
        assert_eq!(screen_a.bracketed_paste_mode(), screen_b.bracketed_paste_mode());
        assert_eq!(screen_a.take_pending_replies().len(), screen_b.take_pending_replies().len());
    }

    #[test]
    fn accept_raw_caps_at_max_bytes() {
        let mut screen = PaneScreen::new(10);
        screen.accept_raw(b"0123456789abcdef");
        assert_eq!(screen.raw_bytes().len(), 10);
        assert_eq!(screen.raw_bytes(), b"6789abcdef");
    }
}
