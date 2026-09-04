//! Canonical preferences document model (RFC-023 §3: `preferences.json`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::color_scheme::{BUILTIN_DARK_SCHEME_NAME, BUILTIN_LIGHT_SCHEME_NAME};
use crate::store::envelope::Schema;

pub const SCHEMA: Schema = Schema::Preferences;
pub const CURRENT_VERSION: u32 = 2;

/// Bare palette names written by version 1 documents. They never matched a real
/// scheme, so both palette combos silently collapsed onto the same entry (#1085).
const LEGACY_LIGHT_SCHEME_NAME: &str = "Daybreak";
const LEGACY_DARK_SCHEME_NAME: &str = "Nightfall";

/// Terminal theme mode selection.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TerminalThemeMode {
    System,
    Light,
    Dark,
}

/// Default session folder behavior.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DefaultSessionFolder {
    Home,
    CurrentSession,
    Custom(String),
}

/// Durable user preferences — no workspace layout, connection status, or runtime inventory.
///
/// The payload shape is identical for document versions 1 and 2; version 2 only
/// narrows which palette names are considered valid (see
/// [`PreferencesV1::migrate_color_scheme_names`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreferencesV1 {
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

impl Default for PreferencesV1 {
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

fn default_font() -> String {
    "Monospace 12".into()
}

const fn default_terminal_theme_mode() -> TerminalThemeMode {
    TerminalThemeMode::System
}

fn default_light_color_scheme() -> String {
    BUILTIN_LIGHT_SCHEME_NAME.into()
}

fn default_dark_color_scheme() -> String {
    BUILTIN_DARK_SCHEME_NAME.into()
}

const fn default_scrollback() -> i64 {
    10_000
}

const fn default_true() -> bool {
    true
}

const fn default_session_folder() -> DefaultSessionFolder {
    DefaultSessionFolder::Home
}

const fn default_reconnect_delay_secs() -> u32 {
    3
}

const fn default_paste_guard_threshold() -> usize {
    200
}

impl PreferencesV1 {
    /// Migrate a version 1 document to version 2 by repairing palette names.
    ///
    /// Version 1 defaults used bare `"Daybreak"`/`"Nightfall"` names that no
    /// scheme ever carried, so the preferences combos fell back to the first
    /// entry and saved the same palette for both modes. Returns `true` when a
    /// name changed, so the caller can rewrite the document.
    pub fn migrate_color_scheme_names(&mut self) -> bool {
        let mut light = canonical_scheme_name(&self.light_color_scheme);
        let mut dark = canonical_scheme_name(&self.dark_color_scheme);
        if light == dark {
            light = default_light_color_scheme();
            dark = default_dark_color_scheme();
        }

        let changed = light != self.light_color_scheme || dark != self.dark_color_scheme;
        self.light_color_scheme = light;
        self.dark_color_scheme = dark;
        changed
    }
}

fn canonical_scheme_name(name: &str) -> String {
    match name {
        LEGACY_LIGHT_SCHEME_NAME => BUILTIN_LIGHT_SCHEME_NAME.into(),
        LEGACY_DARK_SCHEME_NAME => BUILTIN_DARK_SCHEME_NAME.into(),
        other => other.into(),
    }
}

// ── Conversions to/from the existing domain type ────────────

impl From<PreferencesV1> for crate::preferences::Preferences {
    fn from(v1: PreferencesV1) -> Self {
        Self {
            font: v1.font,
            terminal_theme_mode: match v1.terminal_theme_mode {
                TerminalThemeMode::System => crate::preferences::TerminalThemeMode::System,
                TerminalThemeMode::Light => crate::preferences::TerminalThemeMode::Light,
                TerminalThemeMode::Dark => crate::preferences::TerminalThemeMode::Dark,
            },
            light_color_scheme: v1.light_color_scheme,
            dark_color_scheme: v1.dark_color_scheme,
            scrollback_lines: v1.scrollback_lines,
            show_headerbar: v1.show_headerbar,
            scroll_on_keystroke: v1.scroll_on_keystroke,
            scroll_on_output: v1.scroll_on_output,
            audible_bell: v1.audible_bell,
            visual_bell: v1.visual_bell,
            smart_clipboard: v1.smart_clipboard,
            trim_trailing_whitespace_on_copy: v1.trim_trailing_whitespace_on_copy,
            default_session_folder: match v1.default_session_folder {
                DefaultSessionFolder::Home => crate::preferences::DefaultSessionFolder::Home,
                DefaultSessionFolder::CurrentSession => {
                    crate::preferences::DefaultSessionFolder::CurrentSession
                }
                DefaultSessionFolder::Custom(s) => {
                    crate::preferences::DefaultSessionFolder::Custom(s)
                }
            },
            keyboard_shortcuts: v1.keyboard_shortcuts,
            auto_start_daemon: v1.auto_start_daemon,
            reconnect_delay_secs: v1.reconnect_delay_secs,
            paste_guard: v1.paste_guard,
            paste_guard_threshold: v1.paste_guard_threshold,
        }
    }
}

impl From<&crate::preferences::Preferences> for PreferencesV1 {
    fn from(prefs: &crate::preferences::Preferences) -> Self {
        Self {
            font: prefs.font.clone(),
            terminal_theme_mode: match prefs.terminal_theme_mode {
                crate::preferences::TerminalThemeMode::System => TerminalThemeMode::System,
                crate::preferences::TerminalThemeMode::Light => TerminalThemeMode::Light,
                crate::preferences::TerminalThemeMode::Dark => TerminalThemeMode::Dark,
            },
            light_color_scheme: prefs.light_color_scheme.clone(),
            dark_color_scheme: prefs.dark_color_scheme.clone(),
            scrollback_lines: prefs.scrollback_lines,
            show_headerbar: prefs.show_headerbar,
            scroll_on_keystroke: prefs.scroll_on_keystroke,
            scroll_on_output: prefs.scroll_on_output,
            audible_bell: prefs.audible_bell,
            visual_bell: prefs.visual_bell,
            smart_clipboard: prefs.smart_clipboard,
            trim_trailing_whitespace_on_copy: prefs.trim_trailing_whitespace_on_copy,
            default_session_folder: match &prefs.default_session_folder {
                crate::preferences::DefaultSessionFolder::Home => DefaultSessionFolder::Home,
                crate::preferences::DefaultSessionFolder::CurrentSession => {
                    DefaultSessionFolder::CurrentSession
                }
                crate::preferences::DefaultSessionFolder::Custom(s) => {
                    DefaultSessionFolder::Custom(s.clone())
                }
            },
            keyboard_shortcuts: prefs.keyboard_shortcuts.clone(),
            auto_start_daemon: prefs.auto_start_daemon,
            reconnect_delay_secs: prefs.reconnect_delay_secs,
            paste_guard: prefs.paste_guard,
            paste_guard_threshold: prefs.paste_guard_threshold,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_scheme::load_color_scheme_by_name;
    use pretty_assertions::assert_eq;

    #[test]
    fn default_palette_names_resolve_to_distinct_schemes() {
        let prefs = PreferencesV1::default();
        let light = load_color_scheme_by_name(&prefs.light_color_scheme)
            .expect("default light palette must name a real scheme");
        let dark = load_color_scheme_by_name(&prefs.dark_color_scheme)
            .expect("default dark palette must name a real scheme");

        assert_eq!(light.name, BUILTIN_LIGHT_SCHEME_NAME);
        assert_eq!(dark.name, BUILTIN_DARK_SCHEME_NAME);
        assert_ne!(light.background, dark.background, "defaults must render differently");
    }

    #[test]
    fn migration_replaces_legacy_bare_names() {
        let mut prefs = PreferencesV1 {
            light_color_scheme: LEGACY_LIGHT_SCHEME_NAME.into(),
            dark_color_scheme: LEGACY_DARK_SCHEME_NAME.into(),
            ..PreferencesV1::default()
        };

        assert!(prefs.migrate_color_scheme_names());
        assert_eq!(prefs.light_color_scheme, BUILTIN_LIGHT_SCHEME_NAME);
        assert_eq!(prefs.dark_color_scheme, BUILTIN_DARK_SCHEME_NAME);
    }

    #[test]
    fn migration_splits_collapsed_palettes() {
        let mut prefs = PreferencesV1 {
            light_color_scheme: BUILTIN_LIGHT_SCHEME_NAME.into(),
            dark_color_scheme: BUILTIN_LIGHT_SCHEME_NAME.into(),
            ..PreferencesV1::default()
        };

        assert!(prefs.migrate_color_scheme_names());
        assert_eq!(prefs.light_color_scheme, BUILTIN_LIGHT_SCHEME_NAME);
        assert_eq!(prefs.dark_color_scheme, BUILTIN_DARK_SCHEME_NAME);
    }

    #[test]
    fn migration_keeps_distinct_custom_palettes() {
        let mut prefs = PreferencesV1 {
            light_color_scheme: "Solarized Light".into(),
            dark_color_scheme: "Solarized Dark".into(),
            ..PreferencesV1::default()
        };

        assert!(!prefs.migrate_color_scheme_names());
        assert_eq!(prefs.light_color_scheme, "Solarized Light");
        assert_eq!(prefs.dark_color_scheme, "Solarized Dark");
    }

    #[test]
    fn migration_is_idempotent() {
        let mut prefs = PreferencesV1 {
            light_color_scheme: LEGACY_LIGHT_SCHEME_NAME.into(),
            dark_color_scheme: LEGACY_LIGHT_SCHEME_NAME.into(),
            ..PreferencesV1::default()
        };

        assert!(prefs.migrate_color_scheme_names());
        assert!(!prefs.migrate_color_scheme_names());
        assert_eq!(prefs.light_color_scheme, BUILTIN_LIGHT_SCHEME_NAME);
        assert_eq!(prefs.dark_color_scheme, BUILTIN_DARK_SCHEME_NAME);
    }
}
