//! Integration tests for color scheme compatibility.
//!
//! Tests that we can load actual Tilix color scheme files from the
//! original repository, and that our format is a strict superset.

use pretty_assertions::assert_eq;
use rttx::color_scheme::*;
use std::fs;

#[test]
fn load_all_tilix_schemes_from_data_dir() {
    // Load schemes from an external Tilix checkout if available.
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir.parent().and_then(std::path::Path::parent).unwrap();
    let tilix_schemes_dir = [
        repo_root.join("third_party/tilix/data/schemes"),
        repo_root.parent().unwrap_or(repo_root).join("tilix/data/schemes"),
    ]
    .into_iter()
    .find(|path| path.exists())
    .unwrap_or_else(|| repo_root.parent().unwrap_or(repo_root).join("tilix/data/schemes"));

    if !tilix_schemes_dir.exists() {
        eprintln!("Skipping: tilix schemes dir not found at {tilix_schemes_dir:?}");
        return;
    }

    let mut loaded = 0;
    let mut failed = Vec::new();

    for entry in fs::read_dir(&tilix_schemes_dir).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            match load_scheme_file(&path) {
                Ok(scheme) => {
                    // Verify basic invariants
                    assert!(!scheme.name.is_empty(), "Scheme name empty: {path:?}");
                    assert_eq!(scheme.palette.len(), 16, "Wrong palette size in {:?}", path);

                    // Verify all palette colors parse
                    let rgba_palette = scheme.palette_rgba();
                    assert_eq!(
                        rgba_palette.len(),
                        16,
                        "Some palette colors failed to parse in {:?}",
                        path
                    );

                    // Roundtrip test
                    let json = serde_json::to_string_pretty(&scheme).unwrap();
                    let roundtripped: ColorScheme = serde_json::from_str(&json).unwrap();
                    assert_eq!(scheme, roundtripped, "Roundtrip failed for {:?}", path);

                    loaded += 1;
                }
                Err(e) => {
                    failed.push((path.clone(), e.to_string()));
                }
            }
        }
    }

    assert!(failed.is_empty(), "Failed to load {} schemes: {:?}", failed.len(), failed);
    assert!(loaded > 0, "No schemes were loaded from {tilix_schemes_dir:?}");
    eprintln!("Successfully loaded and validated {loaded} Tilix color schemes");
}

#[test]
fn scheme_format_is_superset_of_tilix() {
    // Our format should accept Tilix schemes AND our extended fields
    let extended_json = r##"{
        "name": "Extended",
        "comment": "Has all fields",
        "use-theme-colors": false,
        "foreground-color": "#FFFFFF",
        "background-color": "#000000",
        "use-cursor-color": true,
        "cursor-foreground-color": "#FFFFFF",
        "cursor-background-color": "#FF6600",
        "use-highlight-color": true,
        "highlight-foreground-color": "#FFFFFF",
        "highlight-background-color": "#264F78",
        "use-bold-color": true,
        "bold-color": "#E0E0E0",
        "palette": [
            "#2E3436", "#CC0000", "#4E9A06", "#C4A000",
            "#3465A4", "#75507B", "#06989A", "#D3D7CF",
            "#555753", "#EF2929", "#8AE234", "#FCE94F",
            "#729FCF", "#AD7FA8", "#34E2E2", "#EEEEEC"
        ]
    }"##;

    let scheme: ColorScheme = serde_json::from_str(extended_json).unwrap();
    assert!(scheme.use_cursor_color);
    assert!(scheme.use_highlight_color);
    assert!(scheme.use_bold_color);
    assert_eq!(scheme.cursor_bg, "#FF6600");
}

#[test]
fn minimal_tilix_scheme_loads() {
    // Tilix schemes may only have name + palette + use-theme-colors
    let minimal = r##"{
        "name": "Bare Minimum",
        "use-theme-colors": true,
        "palette": [
            "#000", "#111", "#222", "#333",
            "#444", "#555", "#666", "#777",
            "#888", "#999", "#AAA", "#BBB",
            "#CCC", "#DDD", "#EEE", "#FFF"
        ]
    }"##;

    let scheme: ColorScheme = serde_json::from_str(minimal).unwrap();
    assert_eq!(scheme.name, "Bare Minimum");
    assert!(scheme.use_theme_colors);
    assert!(scheme.foreground.is_empty()); // defaulted
    assert!(!scheme.use_cursor_color); // defaulted to false
}
