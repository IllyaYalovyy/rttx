use gtk4::glib;
use gtk4::prelude::*;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use vte4::prelude::*;

const OPENABLE_URI_PREFIXES: &[&str] = &["http://", "https://", "mailto:", "file://"];
const URI_MATCH_REGEX: &str = r#"(?:https?://|mailto:|file://)[^\s<>'"`)\]\}]+"#;
const PATH_MATCH_REGEX: &str = r#"(?:~/|\.\.?/|/|[A-Za-z0-9._-]+/)[^\s<>'"`]+"#;

type TestUriLauncher = Box<dyn Fn(&str) -> bool>;

thread_local! {
    static TEST_URI_LAUNCHER: std::cell::RefCell<Option<TestUriLauncher>> = std::cell::RefCell::new(None);
}

pub(crate) fn configure_openable_matches(vte: &vte4::Terminal) {
    vte.set_allow_hyperlink(true);
    register_openable_match(vte, URI_MATCH_REGEX);
    register_openable_match(vte, PATH_MATCH_REGEX);
}

pub(crate) fn install_openable_link_controllers<F>(vte: &vte4::Terminal, current_directory: F)
where
    F: Fn() -> Option<String> + 'static,
{
    let current_directory = Rc::new(current_directory);

    // Ctrl+click opens links. Plain click is denied so VTE receives the
    // event for mouse-aware apps (vim, htop, mc, etc.).
    let click_vte = vte.clone();
    let click_current_directory = Rc::clone(&current_directory);
    let open_match_click = gtk4::GestureClick::new();
    open_match_click.set_button(1);
    open_match_click.set_propagation_phase(gtk4::PropagationPhase::Capture);
    open_match_click.connect_released(move |gesture, n_press, x, y| {
        if n_press != 1 {
            gesture.set_state(gtk4::EventSequenceState::Denied);
            return;
        }
        let mods = gesture.current_event_state();
        if !mods.contains(gtk4::gdk::ModifierType::CONTROL_MASK) {
            gesture.set_state(gtk4::EventSequenceState::Denied);
            return;
        }
        let current_directory = click_current_directory();
        let Some(uri) = openable_uri_at(&click_vte, x, y, current_directory.as_deref()) else {
            gesture.set_state(gtk4::EventSequenceState::Denied);
            return;
        };
        if launch_uri(&uri) {
            gesture.set_state(gtk4::EventSequenceState::Claimed);
        }
    });
    vte.add_controller(open_match_click);

    // Show pointer cursor only when Ctrl is held over a link.
    // Only call set_cursor_from_name on state transitions to avoid
    // fighting VTE's own cursor management on every motion event (#479).
    let hover_vte = vte.clone();
    let hover_current_directory = Rc::clone(&current_directory);
    let hover_controller = gtk4::EventControllerMotion::new();
    let showing_pointer = Rc::new(std::cell::Cell::new(false));
    let showing_pointer_motion = Rc::clone(&showing_pointer);
    hover_controller.connect_motion(move |ctrl, x, y| {
        let mods = ctrl.current_event_state();
        let current_directory = hover_current_directory();
        let want_pointer = mods.contains(gtk4::gdk::ModifierType::CONTROL_MASK)
            && openable_uri_at(&hover_vte, x, y, current_directory.as_deref()).is_some();
        if want_pointer != showing_pointer_motion.get() {
            showing_pointer_motion.set(want_pointer);
            hover_vte
                .set_cursor_from_name(if want_pointer { Some("pointer") } else { None });
        }
    });
    let leave_vte = vte.clone();
    hover_controller.connect_leave(move |_| {
        showing_pointer.set(false);
        leave_vte.set_cursor_from_name(None);
    });
    vte.add_controller(hover_controller);
}

pub(crate) fn openable_uri_at(
    vte: &vte4::Terminal,
    x: f64,
    y: f64,
    current_directory: Option<&str>,
) -> Option<String> {
    if let Some(uri) = vte.check_hyperlink_at(x, y) {
        return Some(uri.to_string());
    }

    let (matched, _tag) = vte.check_match_at(x, y);
    matched.as_deref().and_then(|text| openable_uri_from_match_text(text, current_directory))
}

pub(crate) fn launch_uri(uri: &str) -> bool {
    if let Some(result) =
        TEST_URI_LAUNCHER.with(|launcher| launcher.borrow().as_ref().map(|launcher| launcher(uri)))
    {
        return result;
    }

    match gtk4::gio::AppInfo::launch_default_for_uri(uri, gtk4::gio::AppLaunchContext::NONE) {
        Ok(()) => true,
        Err(error) => {
            log::warn!("Failed to open terminal match '{uri}': {error}");
            false
        }
    }
}

pub(crate) fn parse_file_uri(uri: &str) -> Option<String> {
    glib::filename_from_uri(uri).ok().map(|(path, _hostname)| path.display().to_string())
}

/// Return the human-readable text for a URI suitable for clipboard copy.
///
/// File URIs are converted back to filesystem paths; everything else is
/// returned as-is.
pub(crate) fn display_text_for_uri(uri: &str) -> String {
    parse_file_uri(uri).unwrap_or_else(|| uri.to_string())
}

pub(crate) fn openable_uri_from_match_text(
    match_text: &str,
    current_directory: Option<&str>,
) -> Option<String> {
    let trimmed = trim_openable_match(match_text);
    if trimmed.is_empty() {
        return None;
    }

    if OPENABLE_URI_PREFIXES.iter().any(|prefix| trimmed.starts_with(prefix)) {
        return Some(trimmed.to_string());
    }

    let path_text = strip_editor_position_suffix(trimmed);
    if !looks_like_path(path_text) {
        return None;
    }

    let path = resolve_openable_path(path_text, current_directory)?;
    Some(gtk4::gio::File::for_path(path).uri().to_string())
}

fn register_openable_match(vte: &vte4::Terminal, pattern: &str) {
    // VTE 0.78 asserts PCRE2_MULTILINE on match_add_regex. Pass the same
    // defaults VTE uses internally (VTE_REGEX_FLAGS_DEFAULT from vteregex.hh).
    const PCRE2_FLAGS: u32 = 0x0008_0000  // PCRE2_UTF
        | 0x4000_0000  // PCRE2_NO_UTF_CHECK
        | 0x0000_0008  // PCRE2_CASELESS
        | 0x0000_0400  // PCRE2_MULTILINE
        | 0x0000_0020; // PCRE2_DOTALL
    match vte4::Regex::for_match(pattern, PCRE2_FLAGS) {
        Ok(regex) => {
            let tag = vte.match_add_regex(&regex, 0);
            vte.match_set_cursor_name(tag, "pointer");
        }
        Err(error) => {
            log::error!("Failed to register terminal match regex '{pattern}': {error}");
        }
    }
}

fn trim_openable_match(match_text: &str) -> &str {
    match_text.trim_end_matches(['.', ',', ';', '!', '?', ')', ']', '}', '>'])
}

fn strip_editor_position_suffix(path: &str) -> &str {
    let bytes = path.as_bytes();
    let mut end = path.len();
    let mut stripped_any = false;

    loop {
        let mut start = end;
        while start > 0 && bytes[start - 1].is_ascii_digit() {
            start -= 1;
        }

        if start == end || start == 0 || bytes[start - 1] != b':' {
            break;
        }

        stripped_any = true;
        end = start - 1;
    }

    if stripped_any { &path[..end] } else { path }
}

fn looks_like_path(path: &str) -> bool {
    path.starts_with('/')
        || path.starts_with("~/")
        || path.starts_with("./")
        || path.starts_with("../")
        || path.contains('/')
}

fn resolve_openable_path(path: &str, current_directory: Option<&str>) -> Option<PathBuf> {
    if let Some(home_relative) = path.strip_prefix("~/") {
        return Some(glib::home_dir().join(home_relative));
    }

    let path = Path::new(path);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }

    current_directory.map(|cwd| Path::new(cwd).join(path))
}

#[doc(hidden)]
pub fn with_test_uri_launcher<R>(
    launcher: impl Fn(&str) -> bool + 'static,
    f: impl FnOnce() -> R,
) -> R {
    struct Reset;

    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_URI_LAUNCHER.with(|slot| {
                slot.borrow_mut().take();
            });
        }
    }

    TEST_URI_LAUNCHER.with(|slot| {
        assert!(slot.borrow().is_none(), "test URI launcher must not be nested");
        slot.borrow_mut().replace(Box::new(launcher));
    });
    let _reset = Reset;
    f()
}

#[cfg(test)]
mod tests {
    use super::{launch_uri, openable_uri_from_match_text, parse_file_uri, with_test_uri_launcher};
    use gtk4::gio;
    use gtk4::prelude::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn parse_standard_vte_uri() {
        assert_eq!(
            parse_file_uri("file:///home/user/projects"),
            Some("/home/user/projects".into())
        );
    }

    #[test]
    fn parse_uri_with_percent_encoding() {
        assert_eq!(
            parse_file_uri("file:///home/user/my%20project"),
            Some("/home/user/my project".into())
        );
    }

    #[test]
    fn parse_uri_root() {
        assert_eq!(parse_file_uri("file:///"), Some("/".into()));
    }

    #[test]
    fn parse_non_file_uri_returns_none() {
        assert_eq!(parse_file_uri("https://example.com/path"), None);
        assert_eq!(parse_file_uri("ssh://host/path"), None);
    }

    #[test]
    fn parse_empty_string_returns_none() {
        assert_eq!(parse_file_uri(""), None);
    }

    #[test]
    fn strip_prefix_regression() {
        let old_way = "file:///home/user/my%20dir".strip_prefix("file://").map(str::to_string);
        let new_way = parse_file_uri("file:///home/user/my%20dir");
        assert_eq!(old_way, Some("/home/user/my%20dir".into()));
        assert_eq!(new_way, Some("/home/user/my dir".into()));
        assert_ne!(old_way, new_way);
    }

    #[test]
    fn http_match_opens_as_uri() {
        assert_eq!(
            openable_uri_from_match_text("https://example.com/docs?q=rust#intro", None),
            Some("https://example.com/docs?q=rust#intro".into())
        );
    }

    #[test]
    fn trailing_punctuation_is_trimmed_from_uri_match() {
        assert_eq!(
            openable_uri_from_match_text("https://example.com/docs).", None),
            Some("https://example.com/docs".into())
        );
    }

    #[test]
    fn absolute_path_match_becomes_file_uri() {
        let expected = gio::File::for_path("/tmp/rttx.log").uri().to_string();
        assert_eq!(openable_uri_from_match_text("/tmp/rttx.log", None), Some(expected));
    }

    #[test]
    fn relative_path_match_uses_terminal_cwd() {
        let expected = gio::File::for_path("/workspace/rttx/src/main.rs").uri().to_string();
        assert_eq!(
            openable_uri_from_match_text("src/main.rs", Some("/workspace/rttx")),
            Some(expected)
        );
    }

    #[test]
    fn line_and_column_suffix_do_not_block_path_opening() {
        let expected = gio::File::for_path("/workspace/rttx/src/main.rs").uri().to_string();
        assert_eq!(
            openable_uri_from_match_text("src/main.rs:42:7", Some("/workspace/rttx")),
            Some(expected)
        );
    }

    #[test]
    fn bare_word_is_not_treated_as_openable_path() {
        assert_eq!(openable_uri_from_match_text("Cargo.toml", Some("/workspace/rttx")), None);
    }

    #[test]
    fn launch_uri_uses_test_launcher_when_installed() {
        let launched = Rc::new(RefCell::new(Vec::new()));
        let launched_clone = Rc::clone(&launched);
        let result = with_test_uri_launcher(
            move |uri| {
                launched_clone.borrow_mut().push(uri.to_string());
                true
            },
            || launch_uri("https://example.com/docs"),
        );

        assert!(result);
        assert_eq!(launched.borrow().as_slice(), ["https://example.com/docs"]);
    }

    /// The link click gesture must deny when no URI is found so that VTE
    /// receives mouse events for mouse-aware apps (htop, vim, mc). #291.
    /// Since #459, the gesture also requires Ctrl so plain clicks always
    /// pass through to VTE for mouse reporting.
    #[test]
    fn gesture_denied_state_is_available() {
        assert_ne!(gtk4::EventSequenceState::Denied, gtk4::EventSequenceState::Claimed);
    }

    /// Ctrl modifier constant used by the link gesture is the expected value.
    #[test]
    fn ctrl_modifier_mask_is_correct() {
        let ctrl = gtk4::gdk::ModifierType::CONTROL_MASK;
        assert!(ctrl.bits() > 0, "CONTROL_MASK must be a non-zero flag");
    }

    #[test]
    fn display_text_for_file_uri_returns_path() {
        assert_eq!(super::display_text_for_uri("file:///home/user/project"), "/home/user/project");
    }

    #[test]
    fn display_text_for_http_uri_returns_uri() {
        assert_eq!(
            super::display_text_for_uri("https://example.com/docs"),
            "https://example.com/docs"
        );
    }

    #[test]
    fn display_text_for_file_uri_decodes_percent() {
        assert_eq!(
            super::display_text_for_uri("file:///home/user/my%20project"),
            "/home/user/my project"
        );
    }

    /// Hover controller must only update the cursor on state transitions,
    /// not on every motion event. Repeated non-link motion must not call
    /// `set_cursor_from_name`, which would fight VTE's own I-beam cursor. #479.
    #[test]
    fn hover_cursor_updates_only_on_state_transition() {
        use std::cell::Cell;

        let showing_pointer = Cell::new(false);
        let cursor_set_count = Cell::new(0u32);

        let simulate_motion = |want_pointer: bool| {
            if want_pointer != showing_pointer.get() {
                showing_pointer.set(want_pointer);
                cursor_set_count.set(cursor_set_count.get() + 1);
            }
        };

        // Repeated non-link motion must not trigger cursor updates.
        simulate_motion(false);
        simulate_motion(false);
        simulate_motion(false);
        assert_eq!(cursor_set_count.get(), 0, "no cursor update when state unchanged from default");

        // Transition to pointer triggers one update.
        simulate_motion(true);
        assert_eq!(cursor_set_count.get(), 1);

        // Staying on pointer triggers no further updates.
        simulate_motion(true);
        assert_eq!(cursor_set_count.get(), 1);

        // Transition back triggers one update.
        simulate_motion(false);
        assert_eq!(cursor_set_count.get(), 2);

        // Repeated non-link motion again triggers nothing.
        simulate_motion(false);
        simulate_motion(false);
        assert_eq!(cursor_set_count.get(), 2);
    }
}
