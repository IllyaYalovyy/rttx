/// User preferences, persisted as JSON in `XDG_CONFIG_HOME/rttx/preferences.json`.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::color_scheme;
use crate::config;

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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PaneNavigationKeys {
    #[default]
    AltArrow,
    CtrlShiftArrow,
}

impl PaneNavigationKeys {
    /// GTK accelerator strings for (left, right, up, down).
    #[must_use]
    pub const fn accels(&self) -> (&str, &str, &str, &str) {
        match self {
            Self::AltArrow => ("<Alt>Left", "<Alt>Right", "<Alt>Up", "<Alt>Down"),
            Self::CtrlShiftArrow => {
                ("<Ctrl><Shift>Left", "<Ctrl><Shift>Right", "<Ctrl><Shift>Up", "<Ctrl><Shift>Down")
            }
        }
    }
}

const fn default_session_folder() -> DefaultSessionFolder {
    DefaultSessionFolder::Home
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Preferences {
    #[serde(default = "default_font")]
    pub font: String,
    /// Legacy single-scheme preference kept for backward-compatible loading.
    #[serde(default = "default_color_scheme")]
    pub color_scheme: String,
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
    #[serde(default = "default_session_folder")]
    pub default_session_folder: DefaultSessionFolder,
    #[serde(default)]
    pub pane_navigation_keys: PaneNavigationKeys,
}

fn default_font() -> String {
    "Monospace 12".into()
}
fn default_color_scheme() -> String {
    "default".into()
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
impl Default for Preferences {
    fn default() -> Self {
        Self {
            font: default_font(),
            color_scheme: default_color_scheme(),
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
            default_session_folder: default_session_folder(),
            pane_navigation_keys: PaneNavigationKeys::default(),
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
    #[serde(default = "default_color_scheme")]
    color_scheme: String,
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
    #[serde(default = "default_session_folder")]
    default_session_folder: DefaultSessionFolder,
    #[serde(default)]
    pane_navigation_keys: PaneNavigationKeys,
}

impl From<PreferencesDisk> for Preferences {
    fn from(raw: PreferencesDisk) -> Self {
        let legacy_override = if raw.color_scheme == default_color_scheme() {
            None
        } else {
            Some(raw.color_scheme.clone())
        };

        Self {
            font: raw.font,
            color_scheme: raw.color_scheme,
            terminal_theme_mode: raw.terminal_theme_mode,
            light_color_scheme: raw
                .light_color_scheme
                .or_else(|| legacy_override.clone())
                .unwrap_or_else(default_light_color_scheme),
            dark_color_scheme: raw
                .dark_color_scheme
                .or(legacy_override)
                .unwrap_or_else(default_dark_color_scheme),
            scrollback_lines: raw.scrollback_lines,
            show_headerbar: raw.show_headerbar,
            scroll_on_keystroke: raw.scroll_on_keystroke,
            scroll_on_output: raw.scroll_on_output,
            audible_bell: raw.audible_bell,
            visual_bell: raw.visual_bell,
            smart_clipboard: raw.smart_clipboard,
            default_session_folder: raw.default_session_folder,
            pane_navigation_keys: raw.pane_navigation_keys,
        }
    }
}

fn parse_preferences_json(data: &str) -> Preferences {
    serde_json::from_str::<PreferencesDisk>(data).map(Into::into).unwrap_or_default()
}

fn prefs_path() -> PathBuf {
    let mut path = config::config_dir_path();
    path.push("preferences.json");
    path
}

#[must_use]
pub fn load() -> Preferences {
    let path = prefs_path();
    std::fs::read_to_string(path)
        .map_or_else(|_| Preferences::default(), |data| parse_preferences_json(&data))
}

pub fn save(prefs: &Preferences) -> Result<(), Box<dyn std::error::Error>> {
    let path = prefs_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(prefs)?;
    std::fs::write(&path, json)?;
    Ok(())
}

#[must_use]
pub fn load_from(path: &std::path::Path) -> Preferences {
    std::fs::read_to_string(path)
        .map_or_else(|_| Preferences::default(), |data| parse_preferences_json(&data))
}

pub fn save_to(
    prefs: &Preferences,
    path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(prefs)?;
    std::fs::write(path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    use tempfile::TempDir;

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
    }

    #[test]
    fn roundtrip_via_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("prefs.json");
        let prefs = Preferences {
            font: "JetBrains Mono 14".into(),
            scrollback_lines: 5000,
            ..Default::default()
        };
        save_to(&prefs, &path).unwrap();
        let loaded = load_from(&path);
        assert_eq!(prefs, loaded);
    }

    #[test]
    fn missing_file_returns_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.json");
        assert_eq!(load_from(&path), Preferences::default());
    }

    #[test]
    fn corrupt_json_returns_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("prefs.json");
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(load_from(&path), Preferences::default());
    }

    #[test]
    fn partial_json_fills_defaults() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("prefs.json");
        std::fs::write(&path, r#"{"font": "Hack 10"}"#).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.font, "Hack 10");
        assert_eq!(loaded.light_color_scheme, color_scheme::BUILTIN_LIGHT_SCHEME_NAME);
        assert_eq!(loaded.dark_color_scheme, color_scheme::BUILTIN_DARK_SCHEME_NAME);
        assert_eq!(loaded.scrollback_lines, 10000);
        assert!(loaded.show_headerbar);
    }

    #[test]
    fn negative_scrollback_roundtrips() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("prefs.json");
        let prefs = Preferences { scrollback_lines: -1, ..Default::default() };
        save_to(&prefs, &path).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.scrollback_lines, -1);
    }

    #[test]
    fn empty_font_roundtrips() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("prefs.json");
        let prefs = Preferences { font: String::new(), ..Default::default() };
        save_to(&prefs, &path).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.font, "");
    }

    #[test]
    fn legacy_single_color_scheme_populates_light_and_dark() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("prefs.json");
        std::fs::write(&path, r#"{"color_scheme": "Solarized Dark"}"#).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.light_color_scheme, "Solarized Dark");
        assert_eq!(loaded.dark_color_scheme, "Solarized Dark");
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
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("prefs.json");
        std::fs::write(&path, "{}").unwrap();
        let loaded = load_from(&path);
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
        let dir = TempDir::new().unwrap();

        for folder in [
            DefaultSessionFolder::Home,
            DefaultSessionFolder::CurrentSession,
            DefaultSessionFolder::Custom("/home/user/dev".into()),
        ] {
            let path = dir.path().join("prefs.json");
            let prefs =
                Preferences { default_session_folder: folder.clone(), ..Default::default() };
            save_to(&prefs, &path).unwrap();
            let loaded = load_from(&path);
            assert_eq!(loaded.default_session_folder, prefs.default_session_folder);
        }
    }

    #[test]
    fn missing_session_folder_defaults_to_home() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("prefs.json");
        std::fs::write(&path, "{}").unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.default_session_folder, DefaultSessionFolder::Home);
    }

    #[test]
    fn pane_navigation_keys_defaults_to_alt_arrow() {
        let prefs = Preferences::default();
        assert_eq!(prefs.pane_navigation_keys, PaneNavigationKeys::AltArrow);
    }

    #[test]
    fn pane_navigation_keys_roundtrips() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("prefs.json");
        let prefs = Preferences {
            pane_navigation_keys: PaneNavigationKeys::CtrlShiftArrow,
            ..Default::default()
        };
        save_to(&prefs, &path).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.pane_navigation_keys, PaneNavigationKeys::CtrlShiftArrow);
    }

    #[test]
    fn missing_pane_navigation_keys_defaults_to_alt_arrow() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("prefs.json");
        std::fs::write(&path, "{}").unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.pane_navigation_keys, PaneNavigationKeys::AltArrow);
    }
}
