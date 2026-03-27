/// Test helpers and fixtures for rttx tests.
///
/// Provides builder patterns for constructing test data, temporary
/// directory fixtures, and GTK initialization helpers.
use crate::color_scheme::ColorScheme;
use crate::session::layout::{LayoutNode, SessionState, SplitOrientation, WindowState};
use std::path::{Path, PathBuf};

// ── Layout builders ──────────────────────────────────────────────

/// Build a terminal node with a deterministic UUID.
pub fn term(id: &str) -> LayoutNode {
    LayoutNode::Terminal { uuid: id.to_string(), profile: None, cwd: None, custom_title: None }
}

/// Build a terminal node with all fields populated.
pub fn term_full(id: &str, cwd: &str, title: &str) -> LayoutNode {
    LayoutNode::Terminal {
        uuid: id.to_string(),
        profile: Some("default".into()),
        cwd: Some(cwd.into()),
        custom_title: Some(title.into()),
    }
}

/// Build a horizontal split.
pub fn hsplit(first: LayoutNode, second: LayoutNode) -> LayoutNode {
    LayoutNode::Split {
        orientation: SplitOrientation::Horizontal,
        ratio: 0.5,
        first: Box::new(first),
        second: Box::new(second),
    }
}

/// Build a vertical split.
pub fn vsplit(first: LayoutNode, second: LayoutNode) -> LayoutNode {
    LayoutNode::Split {
        orientation: SplitOrientation::Vertical,
        ratio: 0.5,
        first: Box::new(first),
        second: Box::new(second),
    }
}

/// Build a split with a custom ratio.
pub fn split_ratio(
    orientation: SplitOrientation,
    ratio: f64,
    first: LayoutNode,
    second: LayoutNode,
) -> LayoutNode {
    LayoutNode::Split { orientation, ratio, first: Box::new(first), second: Box::new(second) }
}

/// Build a session with a given layout.
pub fn session(id: &str, name: &str, layout: LayoutNode) -> SessionState {
    SessionState {
        uuid: id.to_string(),
        name: name.to_string(),
        layout,
        terminal_recovery: Default::default(),
        active_terminal_uuid: None,
        input_sync: false,
        mode: Default::default(),
        runtime: Default::default(),
    }
}

/// Build a window state from sessions.
pub fn window_state(sessions: Vec<SessionState>) -> WindowState {
    WindowState { active_session_index: 0, sessions, ..WindowState::default() }
}

// ── Color scheme builder ─────────────────────────────────────────

/// Standard Tango-like test palette.
pub const TEST_PALETTE: [&str; 16] = [
    "#2E3436", "#CC0000", "#4E9A06", "#C4A000", "#3465A4", "#75507B", "#06989A", "#D3D7CF",
    "#555753", "#EF2929", "#8AE234", "#FCE94F", "#729FCF", "#AD7FA8", "#34E2E2", "#EEEEEC",
];

pub fn test_scheme(name: &str) -> ColorScheme {
    ColorScheme {
        name: name.into(),
        comment: String::new(),
        use_theme_colors: false,
        foreground: "#FFFFFF".into(),
        background: "#000000".into(),
        palette: TEST_PALETTE.iter().map(|s| s.to_string()).collect(),
        use_cursor_color: false,
        cursor_fg: String::new(),
        cursor_bg: String::new(),
        use_highlight_color: false,
        highlight_fg: String::new(),
        highlight_bg: String::new(),
        use_bold_color: false,
        bold_color: String::new(),
    }
}

pub fn test_scheme_full() -> ColorScheme {
    ColorScheme {
        name: "Full Test".into(),
        comment: "All features enabled".into(),
        use_theme_colors: false,
        foreground: "#FFFFFF".into(),
        background: "#1A1A2E".into(),
        palette: TEST_PALETTE.iter().map(|s| s.to_string()).collect(),
        use_cursor_color: true,
        cursor_fg: "#FFFFFF".into(),
        cursor_bg: "#FF6600".into(),
        use_highlight_color: true,
        highlight_fg: "#FFFFFF".into(),
        highlight_bg: "#264F78".into(),
        use_bold_color: true,
        bold_color: "#E0E0E0".into(),
    }
}

// ── GTK init for tests ───────────────────────────────────────────

static GTK_INIT: std::sync::Once = std::sync::Once::new();
static GTK_AVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Initialize GTK once for the entire test binary. Returns `true` if a
/// display is available and GTK widgets can be created.
///
/// Catches panics from `gtk4::init()` (e.g. "two different threads" in CI
/// containers) so the `Once` never poisons and tests skip gracefully.
///
/// Keeps a leaked reference to the default GDK display so it survives
/// between test functions — without this, dropping all widgets from one
/// test module can close the Broadway connection, causing SIGSEGV in the
/// next module.
pub fn ensure_gtk() -> bool {
    GTK_INIT.call_once(|| {
        set_env("GTK_A11Y", "none");
        let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gtk4::init().is_ok()))
            .unwrap_or(false);
        if ok {
            if let Some(display) = gtk4::gdk::Display::default() {
                std::mem::forget(display);
            }
        }
        GTK_AVAILABLE.store(ok, std::sync::atomic::Ordering::Relaxed);
    });
    GTK_AVAILABLE.load(std::sync::atomic::Ordering::Relaxed)
}

// ── Environment helpers ──────────────────────────────────────────

/// Sets an environment variable in tests.
///
/// # Safety
/// GTK requires all calls to originate from the main thread, so tests that
/// reach this function are single-threaded. No other thread reads the env
/// concurrently during test setup.
#[allow(unsafe_code)]
pub fn set_env(key: &str, value: impl AsRef<std::ffi::OsStr>) {
    unsafe { std::env::set_var(key, value) }
}

/// Removes an environment variable in tests. Same threading guarantee as
/// [`set_env`].
#[allow(unsafe_code)]
pub fn remove_env(key: &str) {
    unsafe { std::env::remove_var(key) }
}

// ── Persistence helpers ──────────────────────────────────────────

/// Save a window state to a temp directory (bypasses glib config dir).
pub fn save_state_to(dir: &Path, state: &WindowState) -> Result<(), Box<dyn std::error::Error>> {
    let sessions_dir = dir.join(crate::config::CONFIG_DIR).join("sessions");
    std::fs::create_dir_all(&sessions_dir)?;
    let path = sessions_dir.join("window-state.json");
    let json = serde_json::to_string_pretty(state)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Load a window state from a temp directory (bypasses glib config dir).
pub fn load_state_from(dir: &Path) -> WindowState {
    let path = dir.join(crate::config::CONFIG_DIR).join("sessions").join("window-state.json");
    match std::fs::read_to_string(path) {
        Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
        Err(_) => WindowState::default(),
    }
}

/// Save a color scheme to a temp directory.
pub fn save_scheme_to(
    dir: &Path,
    scheme: &ColorScheme,
    filename: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(filename);
    crate::color_scheme::save_color_scheme(scheme, &path)?;
    Ok(path)
}

// ── Tilix compatibility JSON samples ─────────────────────────────

/// A real Tilix color scheme JSON string for compatibility testing.
pub const TILIX_TANGO_JSON: &str = r##"{
    "name": "Tango",
    "comment": "Based on the Tango color palette",
    "use-theme-colors": false,
    "foreground-color": "#EEEEEC",
    "background-color": "#2E3436",
    "palette": [
        "#2E3436", "#CC0000", "#4E9A06", "#C4A000",
        "#3465A4", "#75507B", "#06989A", "#D3D7CF",
        "#555753", "#EF2929", "#8AE234", "#FCE94F",
        "#729FCF", "#AD7FA8", "#34E2E2", "#EEEEEC"
    ]
}"##;

pub const TILIX_SOLARIZED_JSON: &str = r##"{
    "name": "Solarized Dark",
    "comment": "Precision colors for machines and people",
    "use-theme-colors": false,
    "foreground-color": "#839496",
    "background-color": "#002B36",
    "use-cursor-color": true,
    "cursor-foreground-color": "#002B36",
    "cursor-background-color": "#839496",
    "use-highlight-color": true,
    "highlight-foreground-color": "#002B36",
    "highlight-background-color": "#268BD2",
    "use-bold-color": true,
    "bold-color": "#93A1A1",
    "palette": [
        "#073642", "#DC322F", "#859900", "#B58900",
        "#268BD2", "#D33682", "#2AA198", "#EEE8D5",
        "#002B36", "#CB4B16", "#586E75", "#657B83",
        "#839496", "#6C71C4", "#93A1A1", "#FDF6E3"
    ]
}"##;

/// Minimal valid scheme JSON — only required fields.
pub const MINIMAL_SCHEME_JSON: &str = r##"{
    "name": "Minimal",
    "palette": [
        "#000000", "#000000", "#000000", "#000000",
        "#000000", "#000000", "#000000", "#000000",
        "#000000", "#000000", "#000000", "#000000",
        "#000000", "#000000", "#000000", "#000000"
    ]
}"##;
