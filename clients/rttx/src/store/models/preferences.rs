//! Canonical preferences document model (RFC-023 §3: `preferences.json`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::store::envelope::Schema;

pub const SCHEMA: Schema = Schema::Preferences;
pub const CURRENT_VERSION: u32 = 1;

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

/// Pane navigation key binding style.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PaneNavigationKeys {
    AltArrow,
    CtrlShiftArrow,
}

/// Durable user preferences — no workspace layout, connection status, or runtime inventory.
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
    #[serde(default = "default_pane_navigation_keys")]
    pub pane_navigation_keys: PaneNavigationKeys,
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
            pane_navigation_keys: default_pane_navigation_keys(),
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
    "Daybreak".into()
}

fn default_dark_color_scheme() -> String {
    "Nightfall".into()
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

const fn default_pane_navigation_keys() -> PaneNavigationKeys {
    PaneNavigationKeys::AltArrow
}

const fn default_reconnect_delay_secs() -> u32 {
    3
}

const fn default_paste_guard_threshold() -> usize {
    200
}
