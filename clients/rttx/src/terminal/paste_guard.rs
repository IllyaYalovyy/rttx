//! Paste guard: warn before pasting large or multiline text into a terminal.
//!
//! The guard analyses clipboard text and decides whether a confirmation dialog
//! should be shown. The dialog offers Paste, Paste as single line, or Cancel.

/// Result of analysing clipboard text for the paste guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PasteAnalysis {
    pub line_count: usize,
    pub byte_len: usize,
    pub preview: String,
}

/// Whether the paste guard should trigger for the given text.
pub(crate) fn needs_confirmation(text: &str, threshold_bytes: usize) -> bool {
    text.contains('\n') || text.contains('\r') || text.len() > threshold_bytes
}

/// Analyse clipboard text for display in the confirmation dialog.
pub(crate) fn analyse(text: &str) -> PasteAnalysis {
    let line_count = text.lines().count().max(1);
    let preview_limit = 200;
    let preview = if text.len() <= preview_limit {
        text.to_string()
    } else {
        let mut end = preview_limit;
        while end < text.len() && !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &text[..end])
    };
    PasteAnalysis { line_count, byte_len: text.len(), preview }
}

/// Collapse all newlines (LF, CR, CRLF) and whitespace runs into single spaces.
pub(crate) fn flatten_to_single_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;
    for ch in text.chars() {
        if ch == '\n' || ch == '\r' || ch == ' ' || ch == '\t' {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

/// What the paste guard should do after reading the clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PasteGuardDecision {
    /// Paste immediately — text is below the guard threshold.
    Paste,
    /// Show a confirmation dialog before pasting.
    Confirm,
    /// No text on clipboard — let VTE handle non-text content natively.
    FallThroughToVte,
    /// No text on clipboard and the terminal cannot handle non-text content.
    Skip,
}

/// Decide what to do after reading clipboard text for the paste guard.
///
/// `is_direct` is true for direct (VTE-backed) terminals that can handle
/// non-text clipboard content natively, false for managed (daemon-backed)
/// terminals that only accept byte streams.
pub(crate) fn decide(
    clipboard_text: Option<&str>,
    threshold_bytes: usize,
    is_direct: bool,
) -> PasteGuardDecision {
    match clipboard_text {
        Some(text) if !text.is_empty() => {
            if needs_confirmation(text, threshold_bytes) {
                PasteGuardDecision::Confirm
            } else {
                PasteGuardDecision::Paste
            }
        }
        _ => {
            if is_direct {
                PasteGuardDecision::FallThroughToVte
            } else {
                PasteGuardDecision::Skip
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_line_below_threshold_does_not_trigger() {
        assert!(!needs_confirmation("hello", 1024));
    }

    #[test]
    fn multiline_text_triggers() {
        assert!(needs_confirmation("line1\nline2", 1024));
    }

    #[test]
    fn carriage_return_triggers() {
        assert!(needs_confirmation("line1\rline2", 1024));
    }

    #[test]
    fn crlf_triggers() {
        assert!(needs_confirmation("line1\r\nline2", 1024));
    }

    #[test]
    fn large_single_line_triggers() {
        let big = "x".repeat(2000);
        assert!(needs_confirmation(&big, 1024));
    }

    #[test]
    fn exactly_at_threshold_does_not_trigger() {
        let text = "x".repeat(1024);
        assert!(!needs_confirmation(&text, 1024));
    }

    #[test]
    fn empty_text_does_not_trigger() {
        assert!(!needs_confirmation("", 1024));
    }

    #[test]
    fn analyse_counts_lines() {
        let a = analyse("one\ntwo\nthree");
        assert_eq!(a.line_count, 3);
    }

    #[test]
    fn analyse_single_line() {
        let a = analyse("hello");
        assert_eq!(a.line_count, 1);
        assert_eq!(a.byte_len, 5);
        assert_eq!(a.preview, "hello");
    }

    #[test]
    fn analyse_truncates_long_preview() {
        let long = "a".repeat(300);
        let a = analyse(&long);
        assert!(a.preview.len() < 210);
        assert!(a.preview.ends_with('…'));
    }

    #[test]
    fn flatten_collapses_newlines() {
        assert_eq!(flatten_to_single_line("a\nb\nc"), "a b c");
    }

    #[test]
    fn flatten_collapses_crlf() {
        assert_eq!(flatten_to_single_line("a\r\nb\r\nc"), "a b c");
    }

    #[test]
    fn flatten_trims_leading_trailing() {
        assert_eq!(flatten_to_single_line("\n  hello  \n"), "hello");
    }

    #[test]
    fn flatten_consecutive_whitespace() {
        assert_eq!(flatten_to_single_line("a  \n\n  b"), "a b");
    }

    #[test]
    fn flatten_empty() {
        assert_eq!(flatten_to_single_line(""), "");
    }

    #[test]
    fn decide_small_text_pastes_immediately() {
        assert_eq!(decide(Some("hello"), 1024, true), PasteGuardDecision::Paste);
        assert_eq!(decide(Some("hello"), 1024, false), PasteGuardDecision::Paste);
    }

    #[test]
    fn decide_multiline_text_confirms() {
        assert_eq!(decide(Some("a\nb"), 1024, true), PasteGuardDecision::Confirm);
        assert_eq!(decide(Some("a\nb"), 1024, false), PasteGuardDecision::Confirm);
    }

    #[test]
    fn decide_large_text_confirms() {
        let big = "x".repeat(2000);
        assert_eq!(decide(Some(&big), 1024, true), PasteGuardDecision::Confirm);
    }

    #[test]
    fn decide_no_text_direct_falls_through_to_vte() {
        assert_eq!(decide(None, 1024, true), PasteGuardDecision::FallThroughToVte);
        assert_eq!(decide(Some(""), 1024, true), PasteGuardDecision::FallThroughToVte);
    }

    #[test]
    fn decide_no_text_managed_skips() {
        assert_eq!(decide(None, 1024, false), PasteGuardDecision::Skip);
        assert_eq!(decide(Some(""), 1024, false), PasteGuardDecision::Skip);
    }
}
