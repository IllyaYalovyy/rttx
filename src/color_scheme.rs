use gtk4::gdk;
use gtk4::glib;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use crate::config;

pub const BUILTIN_DARK_SCHEME_NAME: &str = "Rttx Nightfall";
pub const BUILTIN_LIGHT_SCHEME_NAME: &str = "Rttx Daybreak";

/// A color scheme definition, compatible with Tilix JSON format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ColorScheme {
    pub name: String,
    #[serde(default)]
    pub comment: String,
    #[serde(rename = "use-theme-colors", default)]
    pub use_theme_colors: bool,
    #[serde(rename = "foreground-color", default)]
    pub foreground: String,
    #[serde(rename = "background-color", default)]
    pub background: String,
    pub palette: Vec<String>,
    #[serde(rename = "use-cursor-color", default)]
    pub use_cursor_color: bool,
    #[serde(rename = "cursor-foreground-color", default)]
    pub cursor_fg: String,
    #[serde(rename = "cursor-background-color", default)]
    pub cursor_bg: String,
    #[serde(rename = "use-highlight-color", default)]
    pub use_highlight_color: bool,
    #[serde(rename = "highlight-foreground-color", default)]
    pub highlight_fg: String,
    #[serde(rename = "highlight-background-color", default)]
    pub highlight_bg: String,
    #[serde(rename = "use-bold-color", default)]
    pub use_bold_color: bool,
    #[serde(rename = "bold-color", default)]
    pub bold_color: String,
}

impl ColorScheme {
    #[must_use]
    pub fn parse_color(hex: &str) -> Option<gdk::RGBA> {
        if hex.is_empty() {
            return None;
        }
        gdk::RGBA::parse(hex).ok()
    }

    #[must_use]
    pub fn foreground_rgba(&self) -> Option<gdk::RGBA> {
        Self::parse_color(&self.foreground)
    }

    #[must_use]
    pub fn background_rgba(&self) -> Option<gdk::RGBA> {
        Self::parse_color(&self.background)
    }

    #[must_use]
    pub fn palette_rgba(&self) -> Vec<gdk::RGBA> {
        self.palette.iter().filter_map(|c| Self::parse_color(c)).collect()
    }
}

fn scheme_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for dir in glib::system_data_dirs() {
        dirs.push(dir.join(config::config_dir_name()).join(config::SCHEMES_DIR));
    }
    dirs.push(config::config_dir_path().join(config::SCHEMES_DIR));
    dirs
}

#[allow(clippy::too_many_arguments)]
fn builtin_scheme(
    name: &str,
    comment: &str,
    foreground: &str,
    background: &str,
    palette: &[&str; 16],
    cursor_fg: &str,
    cursor_bg: &str,
    highlight_fg: &str,
    highlight_bg: &str,
    bold_color: &str,
) -> ColorScheme {
    ColorScheme {
        name: name.into(),
        comment: comment.into(),
        use_theme_colors: false,
        foreground: foreground.into(),
        background: background.into(),
        palette: palette.iter().map(|c| (*c).to_string()).collect(),
        use_cursor_color: true,
        cursor_fg: cursor_fg.into(),
        cursor_bg: cursor_bg.into(),
        use_highlight_color: true,
        highlight_fg: highlight_fg.into(),
        highlight_bg: highlight_bg.into(),
        use_bold_color: true,
        bold_color: bold_color.into(),
    }
}

#[must_use]
pub fn builtin_color_schemes() -> Vec<ColorScheme> {
    const NIGHTFALL: [&str; 16] = [
        "#171B24", "#D46A6A", "#86B97A", "#D7B56D", "#7AA2D6", "#B18AD1", "#68B8C1", "#D9DEE7",
        "#4B5563", "#FF8F88", "#A7D79B", "#F4D48B", "#9BC3FF", "#D4ACFF", "#86DDE8", "#F5F7FA",
    ];
    const DAYBREAK: [&str; 16] = [
        "#1F2430", "#C74E39", "#4F8A4F", "#B0832F", "#3B74C5", "#8A56BF", "#2C8A8E", "#D9D3C7",
        "#5C6370", "#E06C50", "#6AA76A", "#C79A3A", "#4F8EE8", "#A06BE0", "#3AA3A8", "#FFFDF8",
    ];

    vec![
        builtin_scheme(
            BUILTIN_DARK_SCHEME_NAME,
            "A deep graphite terminal with restrained accents and clear ANSI contrast.",
            "#E6E7EB",
            "#11141A",
            &NIGHTFALL,
            "#11141A",
            "#FFB454",
            "#F5F7FA",
            "#2A3A52",
            "#FFFFFF",
        ),
        builtin_scheme(
            BUILTIN_LIGHT_SCHEME_NAME,
            "A warm paper-light terminal designed to sit comfortably inside GNOME light mode.",
            "#243039",
            "#F7F4EC",
            &DAYBREAK,
            "#F7F4EC",
            "#B14D32",
            "#243039",
            "#D9E7FF",
            "#111827",
        ),
    ]
}

#[must_use]
pub fn load_color_schemes() -> Vec<ColorScheme> {
    let mut schemes: BTreeMap<String, ColorScheme> =
        builtin_color_schemes().into_iter().map(|scheme| (scheme.name.clone(), scheme)).collect();
    for dir in scheme_search_dirs() {
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "json") {
                    match load_scheme_file(&path) {
                        Ok(scheme) => {
                            schemes.insert(scheme.name.clone(), scheme);
                        }
                        Err(e) => {
                            log::warn!("Failed to load color scheme {}: {e}", path.display());
                        }
                    }
                }
            }
        }
    }
    schemes.into_values().collect()
}

#[must_use]
pub fn load_color_scheme_by_name(name: &str) -> Option<ColorScheme> {
    load_color_schemes().into_iter().find(|scheme| scheme.name == name)
}

pub fn load_scheme_file(path: &std::path::Path) -> Result<ColorScheme, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    let scheme: ColorScheme = serde_json::from_str(&content)?;
    if scheme.palette.len() != 16 {
        return Err(format!(
            "Color scheme '{}' has {} palette colors, expected 16",
            scheme.name,
            scheme.palette.len()
        )
        .into());
    }
    Ok(scheme)
}

pub fn save_color_scheme(
    scheme: &ColorScheme,
    path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let json = serde_json::to_string_pretty(scheme)?;
    fs::write(path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;

    #[test]
    fn scheme_serialization_roundtrip() {
        let scheme = test_scheme("Roundtrip");
        let json = serde_json::to_string(&scheme).unwrap();
        let deserialized: ColorScheme = serde_json::from_str(&json).unwrap();
        assert_eq!(scheme, deserialized);
    }

    #[test]
    fn full_scheme_roundtrip() {
        let scheme = test_scheme_full();
        let json = serde_json::to_string_pretty(&scheme).unwrap();
        let deserialized: ColorScheme = serde_json::from_str(&json).unwrap();
        assert_eq!(scheme, deserialized);
    }

    #[rstest]
    #[case("#FF0000", 1.0, 0.0, 0.0)]
    #[case("#00FF00", 0.0, 1.0, 0.0)]
    #[case("#0000FF", 0.0, 0.0, 1.0)]
    #[case("#FFFFFF", 1.0, 1.0, 1.0)]
    #[case("#000000", 0.0, 0.0, 0.0)]
    fn parse_hex_colors(#[case] hex: &str, #[case] r: f32, #[case] g: f32, #[case] b: f32) {
        let rgba = ColorScheme::parse_color(hex).unwrap();
        assert!((rgba.red() - r).abs() < 0.02, "red: {} != {}", rgba.red(), r);
        assert!((rgba.green() - g).abs() < 0.02, "green: {} != {}", rgba.green(), g);
        assert!((rgba.blue() - b).abs() < 0.02, "blue: {} != {}", rgba.blue(), b);
    }

    #[rstest]
    #[case("")]
    #[case("not-a-color")]
    #[case("FFFFFF")]
    #[case("#GG0000")]
    fn parse_invalid_colors_returns_none(#[case] input: &str) {
        assert!(ColorScheme::parse_color(input).is_none(), "Should fail for '{input}'");
    }

    #[test]
    fn palette_rgba_returns_16_colors() {
        let scheme = test_scheme("Palette");
        let palette = scheme.palette_rgba();
        assert_eq!(palette.len(), 16);
    }

    #[test]
    fn save_and_load_scheme_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let scheme = test_scheme("SaveLoad");
        let path = save_scheme_to(tmp.path(), &scheme, "test.json").unwrap();
        let loaded = load_scheme_file(&path).unwrap();
        assert_eq!(scheme, loaded);
    }

    #[test]
    fn load_rejects_wrong_palette_size() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut scheme = test_scheme("BadPalette");
        scheme.palette.pop();
        let path = tmp.path().join("bad.json");
        let json = serde_json::to_string(&scheme).unwrap();
        std::fs::write(&path, json).unwrap();
        let result = load_scheme_file(&path);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("16"),
            "Error should mention expected palette size"
        );
    }

    #[test]
    fn load_rejects_invalid_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("garbage.json");
        std::fs::write(&path, "{{not json}}").unwrap();
        assert!(load_scheme_file(&path).is_err());
    }

    #[rstest]
    #[case(TILIX_TANGO_JSON, "Tango", false, false, false)]
    #[case(TILIX_SOLARIZED_JSON, "Solarized Dark", true, true, true)]
    #[case(MINIMAL_SCHEME_JSON, "Minimal", false, false, false)]
    fn tilix_scheme_compatibility(
        #[case] json: &str,
        #[case] expected_name: &str,
        #[case] has_cursor: bool,
        #[case] has_highlight: bool,
        #[case] has_bold: bool,
    ) {
        let scheme: ColorScheme = serde_json::from_str(json).unwrap();
        assert_eq!(scheme.name, expected_name);
        assert_eq!(scheme.palette.len(), 16);
        assert_eq!(scheme.use_cursor_color, has_cursor);
        assert_eq!(scheme.use_highlight_color, has_highlight);
        assert_eq!(scheme.use_bold_color, has_bold);
    }

    #[test]
    fn tilix_solarized_colors_parse() {
        let scheme: ColorScheme = serde_json::from_str(TILIX_SOLARIZED_JSON).unwrap();
        assert!(scheme.foreground_rgba().is_some());
        assert!(scheme.background_rgba().is_some());
        assert_eq!(scheme.palette_rgba().len(), 16);
        assert!(ColorScheme::parse_color(&scheme.cursor_fg).is_some());
        assert!(ColorScheme::parse_color(&scheme.cursor_bg).is_some());
    }

    #[test]
    fn builtin_schemes_are_present_and_valid() {
        let schemes = load_color_schemes();
        let dark = schemes
            .iter()
            .find(|scheme| scheme.name == BUILTIN_DARK_SCHEME_NAME)
            .expect("dark builtin scheme must be present");
        let light = schemes
            .iter()
            .find(|scheme| scheme.name == BUILTIN_LIGHT_SCHEME_NAME)
            .expect("light builtin scheme must be present");

        assert_eq!(dark.palette.len(), 16);
        assert_eq!(light.palette.len(), 16);
        assert!(dark.foreground_rgba().is_some());
        assert!(dark.background_rgba().is_some());
        assert!(light.foreground_rgba().is_some());
        assert!(light.background_rgba().is_some());
    }

    #[test]
    fn load_multiple_schemes_from_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path();

        save_scheme_to(dir, &test_scheme("Zebra"), "zebra.json").unwrap();
        save_scheme_to(dir, &test_scheme("Alpha"), "alpha.json").unwrap();
        save_scheme_to(dir, &test_scheme("Middle"), "middle.json").unwrap();

        std::fs::write(dir.join("readme.txt"), "not a scheme").unwrap();

        let mut schemes = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                if let Ok(s) = load_scheme_file(&path) {
                    schemes.push(s);
                }
            }
        }
        schemes.sort_by(|a, b| a.name.cmp(&b.name));

        assert_eq!(schemes.len(), 3);
        assert_eq!(schemes[0].name, "Alpha");
        assert_eq!(schemes[1].name, "Middle");
        assert_eq!(schemes[2].name, "Zebra");
    }
}
