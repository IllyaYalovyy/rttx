//! Repo-owned PTY exerciser for terminal mode parity testing.
//!
//! Reads line-based commands from stdin and emits the corresponding
//! escape sequences on stdout. Designed to run inside a PTY so that
//! the controlling test harness can verify mode state from the outside.
//!
//! Commands (one per line, case-insensitive):
//! - `SET <mode>` — emit the escape sequence that enables `<mode>`
//! - `RESET <mode>` — emit the escape sequence that disables `<mode>`
//! - `ECHO <hex>` — write raw bytes (hex-encoded) to stdout
//! - `READY` — print `EXERCISER_READY` marker
//! - `QUIT` — exit cleanly
//!
//! Supported modes:
//! - `app_cursor` — DECSET/DECRST 1 (application cursor keys)
//! - `app_keypad` — DECKPAM / DECKPNM (application keypad)
//! - `bracketed_paste` — DECSET/DECRST 2004
//! - `focus_reporting` — DECSET/DECRST 1004
//! - `mouse_1000` — DECSET/DECRST 1000 (basic mouse tracking)
//! - `mouse_1002` — DECSET/DECRST 1002 (button-event tracking)
//! - `mouse_1003` — DECSET/DECRST 1003 (any-event tracking)
//! - `sgr_mouse` — DECSET/DECRST 1006 (SGR mouse encoding)

use std::io::{self, BufRead, Write};

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim().to_ascii_uppercase();
        if line.is_empty() {
            continue;
        }

        if line == "READY" {
            let _ = stdout.write_all(b"EXERCISER_READY\n");
            let _ = stdout.flush();
            continue;
        }
        if line == "QUIT" {
            break;
        }

        let parts: Vec<&str> = line.splitn(2, ' ').collect();
        if parts.len() < 2 {
            continue;
        }

        match parts[0] {
            "SET" => {
                if let Some(seq) = mode_set_sequence(parts[1]) {
                    let _ = stdout.write_all(seq);
                    let _ = stdout.flush();
                }
            }
            "RESET" => {
                if let Some(seq) = mode_reset_sequence(parts[1]) {
                    let _ = stdout.write_all(seq);
                    let _ = stdout.flush();
                }
            }
            "ECHO" => {
                if let Some(bytes) = hex_decode(parts[1]) {
                    let _ = stdout.write_all(&bytes);
                    let _ = stdout.flush();
                }
            }
            _ => {}
        }
    }
}

fn mode_set_sequence(mode: &str) -> Option<&'static [u8]> {
    match mode {
        "APP_CURSOR" => Some(b"\x1b[?1h"),
        "APP_KEYPAD" => Some(b"\x1b="),
        "BRACKETED_PASTE" => Some(b"\x1b[?2004h"),
        "FOCUS_REPORTING" => Some(b"\x1b[?1004h"),
        "MOUSE_1000" => Some(b"\x1b[?1000h"),
        "MOUSE_1002" => Some(b"\x1b[?1002h"),
        "MOUSE_1003" => Some(b"\x1b[?1003h"),
        "SGR_MOUSE" => Some(b"\x1b[?1006h"),
        _ => None,
    }
}

fn mode_reset_sequence(mode: &str) -> Option<&'static [u8]> {
    match mode {
        "APP_CURSOR" => Some(b"\x1b[?1l"),
        "APP_KEYPAD" => Some(b"\x1b>"),
        "BRACKETED_PASTE" => Some(b"\x1b[?2004l"),
        "FOCUS_REPORTING" => Some(b"\x1b[?1004l"),
        "MOUSE_1000" => Some(b"\x1b[?1000l"),
        "MOUSE_1002" => Some(b"\x1b[?1002l"),
        "MOUSE_1003" => Some(b"\x1b[?1003l"),
        "SGR_MOUSE" => Some(b"\x1b[?1006l"),
        _ => None,
    }
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_sequences_cover_all_documented_modes() {
        let modes = [
            "APP_CURSOR",
            "APP_KEYPAD",
            "BRACKETED_PASTE",
            "FOCUS_REPORTING",
            "MOUSE_1000",
            "MOUSE_1002",
            "MOUSE_1003",
            "SGR_MOUSE",
        ];
        for mode in modes {
            assert!(mode_set_sequence(mode).is_some(), "SET must handle {mode}");
            assert!(mode_reset_sequence(mode).is_some(), "RESET must handle {mode}");
        }
    }

    #[test]
    fn hex_decode_valid() {
        assert_eq!(hex_decode("1b5b3f3168"), Some(b"\x1b[?1h".to_vec()));
    }

    #[test]
    fn hex_decode_odd_length_returns_none() {
        assert_eq!(hex_decode("1b5"), None);
    }
}
