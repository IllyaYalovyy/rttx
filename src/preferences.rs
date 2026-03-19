/// User preferences, persisted as JSON in XDG_CONFIG_HOME/rttx/preferences.json.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::config;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Preferences {
    #[serde(default = "default_font")]
    pub font: String,
    #[serde(default = "default_color_scheme")]
    pub color_scheme: String,
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
    #[serde(default = "default_opacity")]
    pub background_opacity: f64,
}

fn default_font() -> String { "Monospace 12".into() }
fn default_color_scheme() -> String { "default".into() }
fn default_scrollback() -> i64 { 10000 }
fn default_true() -> bool { true }
fn default_opacity() -> f64 { 1.0 }

impl Default for Preferences {
    fn default() -> Self {
        Self {
            font: default_font(),
            color_scheme: default_color_scheme(),
            scrollback_lines: default_scrollback(),
            show_headerbar: true,
            scroll_on_keystroke: true,
            scroll_on_output: false,
            audible_bell: true,
            background_opacity: default_opacity(),
        }
    }
}

fn prefs_path() -> PathBuf {
    let mut path = glib::user_config_dir();
    path.push(config::CONFIG_DIR);
    path.push("preferences.json");
    path
}

use gtk4::glib;

pub fn load() -> Preferences {
    let path = prefs_path();
    match std::fs::read_to_string(&path) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
        Err(_) => Preferences::default(),
    }
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

/// Load from a specific path (for testing without glib).
pub fn load_from(path: &std::path::Path) -> Preferences {
    match std::fs::read_to_string(path) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
        Err(_) => Preferences::default(),
    }
}

/// Save to a specific path (for testing without glib).
pub fn save_to(prefs: &Preferences, path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
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
    use rstest::rstest;
    use tempfile::TempDir;

    #[test]
    fn default_preferences_are_sensible() {
        let prefs = Preferences::default();
        assert_eq!(prefs.font, "Monospace 12");
        assert_eq!(prefs.scrollback_lines, 10000);
        assert!(prefs.show_headerbar);
        assert!(prefs.scroll_on_keystroke);
        assert!(!prefs.scroll_on_output);
        assert!((prefs.background_opacity - 1.0).abs() < f64::EPSILON);
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
        assert_eq!(loaded.scrollback_lines, 10000); // default
        assert!(loaded.show_headerbar); // default
    }

    #[rstest]
    #[case(0.0)]
    #[case(0.5)]
    #[case(1.0)]
    fn opacity_roundtrip(#[case] opacity: f64) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("prefs.json");
        let prefs = Preferences { background_opacity: opacity, ..Default::default() };
        save_to(&prefs, &path).unwrap();
        let loaded = load_from(&path);
        assert!((loaded.background_opacity - opacity).abs() < f64::EPSILON);
    }
}
