/// User preferences, persisted as JSON in `XDG_CONFIG_HOME/rttx/preferences.json`.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::color_scheme;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TerminalThemeMode {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DefaultSessionFolder {
    Home,
    CurrentSession,
    Custom(String),
}

const fn default_session_folder() -> DefaultSessionFolder {
    DefaultSessionFolder::Home
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Preferences {
    #[serde(default = "default_font")]
    pub font: String,
    #[serde(default = "default_terminal_theme_mode")]
    pub terminal_theme_mode: TerminalThemeMode,
    #[serde(default = "default_light_color_scheme")]
    pub light_color_scheme: String,
    #[serde(default = "default_dark_color_scheme")]
    pub dark_color_scheme: String,
    #[serde(default = "default_scrollback")]
    pub scrollback_lines: i64,
    #[serde(default = "default_true")]
    pub show_headerbar: bool,
    #[serde(default = "default_true")]
    pub scroll_on_keystroke: bool,
    #[serde(default)]
    pub scroll_on_output: bool,
    #[serde(default = "default_true")]
    pub audible_bell: bool,
    #[serde(default = "default_true")]
    pub visual_bell: bool,
    #[serde(default)]
    pub smart_clipboard: bool,
    #[serde(default)]
    pub trim_trailing_whitespace_on_copy: bool,
    #[serde(default = "default_session_folder")]
    pub default_session_folder: DefaultSessionFolder,
    #[serde(default)]
    pub keyboard_shortcuts: BTreeMap<String, Vec<String>>,
    #[serde(default = "default_true")]
    pub auto_start_daemon: bool,
    #[serde(default = "default_reconnect_delay_secs")]
    pub reconnect_delay_secs: u32,
    #[serde(default = "default_true")]
    pub paste_guard: bool,
    #[serde(default = "default_paste_guard_threshold")]
    pub paste_guard_threshold: usize,
}

fn default_font() -> String {
    "Monospace 12".into()
}
const fn default_terminal_theme_mode() -> TerminalThemeMode {
    TerminalThemeMode::System
}
fn default_light_color_scheme() -> String {
    color_scheme::BUILTIN_LIGHT_SCHEME_NAME.into()
}
fn default_dark_color_scheme() -> String {
    color_scheme::BUILTIN_DARK_SCHEME_NAME.into()
}
const fn default_scrollback() -> i64 {
    10000
}
const fn default_true() -> bool {
    true
}
const fn default_reconnect_delay_secs() -> u32 {
    10
}

const fn default_paste_guard_threshold() -> usize {
    1024
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            font: default_font(),
            terminal_theme_mode: default_terminal_theme_mode(),
            light_color_scheme: default_light_color_scheme(),
            dark_color_scheme: default_dark_color_scheme(),
            scrollback_lines: default_scrollback(),
            show_headerbar: true,
            scroll_on_keystroke: true,
            scroll_on_output: false,
            audible_bell: true,
            visual_bell: true,
            smart_clipboard: false,
            trim_trailing_whitespace_on_copy: false,
            default_session_folder: default_session_folder(),
            keyboard_shortcuts: BTreeMap::new(),
            auto_start_daemon: true,
            reconnect_delay_secs: default_reconnect_delay_secs(),
            paste_guard: true,
            paste_guard_threshold: default_paste_guard_threshold(),
        }
    }
}

impl Preferences {
    #[must_use]
    pub fn effective_color_scheme_name(&self, is_dark: bool) -> &str {
        match self.terminal_theme_mode {
            TerminalThemeMode::System => {
                if is_dark {
                    &self.dark_color_scheme
                } else {
                    &self.light_color_scheme
                }
            }
            TerminalThemeMode::Light => &self.light_color_scheme,
            TerminalThemeMode::Dark => &self.dark_color_scheme,
        }
    }
}

#[derive(Debug, Deserialize)]
struct PreferencesDisk {
    #[serde(default = "default_font")]
    font: String,
    #[serde(default = "default_terminal_theme_mode")]
    terminal_theme_mode: TerminalThemeMode,
    #[serde(default)]
    light_color_scheme: Option<String>,
    #[serde(default)]
    dark_color_scheme: Option<String>,
    #[serde(default = "default_scrollback")]
    scrollback_lines: i64,
    #[serde(default = "default_true")]
    show_headerbar: bool,
    #[serde(default = "default_true")]
    scroll_on_keystroke: bool,
    #[serde(default)]
    scroll_on_output: bool,
    #[serde(default = "default_true")]
    audible_bell: bool,
    #[serde(default = "default_true")]
    visual_bell: bool,
    #[serde(default)]
    smart_clipboard: bool,
    #[serde(default)]
    trim_trailing_whitespace_on_copy: bool,
    #[serde(default = "default_session_folder")]
    default_session_folder: DefaultSessionFolder,
    #[serde(default)]
    keyboard_shortcuts: BTreeMap<String, Vec<String>>,
    #[serde(default = "default_true")]
    auto_start_daemon: bool,
    #[serde(default = "default_reconnect_delay_secs")]
    reconnect_delay_secs: u32,
    #[serde(default = "default_true")]
    paste_guard: bool,
    #[serde(default = "default_paste_guard_threshold")]
    paste_guard_threshold: usize,
}

impl From<PreferencesDisk> for Preferences {
    fn from(raw: PreferencesDisk) -> Self {
        Self {
            font: raw.font,
            terminal_theme_mode: raw.terminal_theme_mode,
            light_color_scheme: raw.light_color_scheme.unwrap_or_else(default_light_color_scheme),
            dark_color_scheme: raw.dark_color_scheme.unwrap_or_else(default_dark_color_scheme),
            scrollback_lines: raw.scrollback_lines,
            show_headerbar: raw.show_headerbar,
            scroll_on_keystroke: raw.scroll_on_keystroke,
            scroll_on_output: raw.scroll_on_output,
            audible_bell: raw.audible_bell,
            visual_bell: raw.visual_bell,
            smart_clipboard: raw.smart_clipboard,
            trim_trailing_whitespace_on_copy: raw.trim_trailing_whitespace_on_copy,
            default_session_folder: raw.default_session_folder,
            keyboard_shortcuts: raw.keyboard_shortcuts,
            auto_start_daemon: raw.auto_start_daemon,
            reconnect_delay_secs: raw.reconnect_delay_secs,
            paste_guard: raw.paste_guard,
            paste_guard_threshold: raw.paste_guard_threshold,
        }
    }
}

/// Parse a raw JSON string into `Preferences`, applying legacy migrations.
///
/// Used only by unit tests to verify backward-compatible deserialization
/// without going through `ClientStore`.
#[cfg(test)]
fn parse_preferences_json(data: &str) -> Preferences {
    serde_json::from_str::<PreferencesDisk>(data).map(Into::into).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn roundtrip(prefs: &Preferences) -> Preferences {
        let json = serde_json::to_string_pretty(prefs).unwrap();
        parse_preferences_json(&json)
    }

    #[test]
    fn default_preferences_are_sensible() {
        let prefs = Preferences::default();
        assert_eq!(prefs.font, "Monospace 12");
        assert_eq!(prefs.terminal_theme_mode, TerminalThemeMode::System);
        assert_eq!(prefs.light_color_scheme, color_scheme::BUILTIN_LIGHT_SCHEME_NAME);
        assert_eq!(prefs.dark_color_scheme, color_scheme::BUILTIN_DARK_SCHEME_NAME);
        assert_eq!(prefs.scrollback_lines, 10000);
        assert!(prefs.show_headerbar);
        assert!(prefs.scroll_on_keystroke);
        assert!(!prefs.scroll_on_output);
        assert!(!prefs.smart_clipboard);
        assert!(!prefs.trim_trailing_whitespace_on_copy);
        assert!(prefs.auto_start_daemon);
        assert_eq!(prefs.reconnect_delay_secs, 10);
        assert!(prefs.paste_guard);
        assert_eq!(prefs.paste_guard_threshold, 1024);
    }

    #[test]
    fn serde_roundtrip() {
        let prefs = Preferences {
            font: "JetBrains Mono 14".into(),
            scrollback_lines: 5000,
            ..Default::default()
        };
        assert_eq!(prefs, roundtrip(&prefs));
    }

    #[test]
    fn corrupt_json_returns_default() {
        assert_eq!(parse_preferences_json("not json"), Preferences::default());
    }

    #[test]
    fn partial_json_fills_defaults() {
        let loaded = parse_preferences_json(r#"{"font": "Hack 10"}"#);
        assert_eq!(loaded.font, "Hack 10");
        assert_eq!(loaded.light_color_scheme, color_scheme::BUILTIN_LIGHT_SCHEME_NAME);
        assert_eq!(loaded.dark_color_scheme, color_scheme::BUILTIN_DARK_SCHEME_NAME);
        assert_eq!(loaded.scrollback_lines, 10000);
        assert!(loaded.show_headerbar);
    }

    #[test]
    fn negative_scrollback_roundtrips() {
        let prefs = Preferences { scrollback_lines: -1, ..Default::default() };
        assert_eq!(roundtrip(&prefs).scrollback_lines, -1);
    }

    #[test]
    fn empty_font_roundtrips() {
        let prefs = Preferences { font: String::new(), ..Default::default() };
        assert_eq!(roundtrip(&prefs).font, "");
    }

    #[test]
    fn effective_color_scheme_tracks_mode() {
        let prefs = Preferences {
            terminal_theme_mode: TerminalThemeMode::System,
            light_color_scheme: "Light".into(),
            dark_color_scheme: "Dark".into(),
            ..Default::default()
        };
        assert_eq!(prefs.effective_color_scheme_name(false), "Light");
        assert_eq!(prefs.effective_color_scheme_name(true), "Dark");

        let prefs = Preferences {
            terminal_theme_mode: TerminalThemeMode::Light,
            light_color_scheme: "Light".into(),
            dark_color_scheme: "Dark".into(),
            ..Default::default()
        };
        assert_eq!(prefs.effective_color_scheme_name(false), "Light");
        assert_eq!(prefs.effective_color_scheme_name(true), "Light");
    }

    #[test]
    fn boolean_defaults_are_correct() {
        let loaded = parse_preferences_json("{}");
        assert!(loaded.show_headerbar, "show_headerbar should default true");
        assert!(loaded.scroll_on_keystroke, "scroll_on_keystroke should default true");
        assert!(!loaded.scroll_on_output, "scroll_on_output should default false");
        assert!(loaded.audible_bell, "audible_bell should default true");
        assert!(loaded.visual_bell, "visual_bell should default true");
    }

    #[test]
    fn default_session_folder_defaults_to_home() {
        let prefs = Preferences::default();
        assert_eq!(prefs.default_session_folder, DefaultSessionFolder::Home);
    }

    #[test]
    fn default_session_folder_roundtrips() {
        for folder in [
            DefaultSessionFolder::Home,
            DefaultSessionFolder::CurrentSession,
            DefaultSessionFolder::Custom("/home/user/dev".into()),
        ] {
            let prefs =
                Preferences { default_session_folder: folder.clone(), ..Default::default() };
            assert_eq!(roundtrip(&prefs).default_session_folder, prefs.default_session_folder);
        }
    }

    #[test]
    fn missing_session_folder_defaults_to_home() {
        let loaded = parse_preferences_json("{}");
        assert_eq!(loaded.default_session_folder, DefaultSessionFolder::Home);
    }

    #[test]
    fn auto_start_daemon_defaults_to_true() {
        assert!(Preferences::default().auto_start_daemon);
    }

    #[test]
    fn auto_start_daemon_roundtrips_false() {
        let prefs = Preferences { auto_start_daemon: false, ..Default::default() };
        assert!(!roundtrip(&prefs).auto_start_daemon);
    }

    #[test]
    fn missing_auto_start_daemon_defaults_to_true() {
        assert!(parse_preferences_json("{}").auto_start_daemon);
    }

    #[test]
    fn reconnect_delay_secs_defaults_to_10() {
        assert_eq!(Preferences::default().reconnect_delay_secs, 10);
    }

    #[test]
    fn reconnect_delay_secs_roundtrips() {
        let prefs = Preferences { reconnect_delay_secs: 30, ..Default::default() };
        assert_eq!(roundtrip(&prefs).reconnect_delay_secs, 30);
    }

    #[test]
    fn missing_reconnect_delay_secs_defaults_to_10() {
        assert_eq!(parse_preferences_json("{}").reconnect_delay_secs, 10);
    }

    #[test]
    fn paste_guard_defaults_to_enabled() {
        let prefs = Preferences::default();
        assert!(prefs.paste_guard);
        assert_eq!(prefs.paste_guard_threshold, 1024);
    }

    #[test]
    fn paste_guard_roundtrips() {
        let prefs =
            Preferences { paste_guard: false, paste_guard_threshold: 4096, ..Default::default() };
        let loaded = roundtrip(&prefs);
        assert!(!loaded.paste_guard);
        assert_eq!(loaded.paste_guard_threshold, 4096);
    }

    #[test]
    fn missing_paste_guard_defaults_to_enabled() {
        let loaded = parse_preferences_json("{}");
        assert!(loaded.paste_guard);
        assert_eq!(loaded.paste_guard_threshold, 1024);
    }
}
