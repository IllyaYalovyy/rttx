/// Integration tests for preferences persistence.
use std::collections::BTreeMap;

use rttx::preferences::{
    self, DefaultSessionFolder, PaneNavigationKeys, Preferences, TerminalThemeMode,
};
use tempfile::TempDir;

#[test]
fn preferences_default_values_are_reasonable() {
    let prefs = Preferences::default();
    assert!(prefs.font.contains("12") || prefs.font.contains("Mono"));
    assert!(prefs.scrollback_lines >= 1000);
}

#[test]
fn preferences_roundtrip_all_fields() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("prefs.json");

    let prefs = Preferences {
        font: "Fira Code 16".into(),
        color_scheme: "Solarized Dark".into(),
        terminal_theme_mode: TerminalThemeMode::Dark,
        light_color_scheme: "Rttx Daybreak".into(),
        dark_color_scheme: "Solarized Dark".into(),
        scrollback_lines: 50000,
        show_headerbar: false,
        scroll_on_keystroke: false,
        scroll_on_output: true,
        audible_bell: false,
        visual_bell: true,
        smart_clipboard: true,
        trim_trailing_whitespace_on_copy: true,
        default_session_folder: DefaultSessionFolder::Custom("/home/user/dev".into()),
        pane_navigation_keys: PaneNavigationKeys::AltArrow,
        keyboard_shortcuts: BTreeMap::new(),
        auto_start_daemon: true,
        reconnect_delay_secs: 10,
        paste_guard: false,
        paste_guard_threshold: 2048,
    };

    preferences::save_to(&prefs, &path).unwrap();
    let loaded = preferences::load_from(&path);
    assert_eq!(prefs, loaded);
}

#[test]
fn preferences_partial_json_uses_defaults_for_missing() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("prefs.json");

    // Only set font — everything else should default
    std::fs::write(&path, r#"{"font": "Hack 10"}"#).unwrap();
    let loaded = preferences::load_from(&path);

    assert_eq!(loaded.font, "Hack 10");
    assert_eq!(loaded.terminal_theme_mode, TerminalThemeMode::System);
    assert_eq!(loaded.light_color_scheme, "Rttx Daybreak");
    assert_eq!(loaded.dark_color_scheme, "Rttx Nightfall");
    assert_eq!(loaded.scrollback_lines, 10000);
    assert!(loaded.show_headerbar);
    assert!(!loaded.smart_clipboard);
}

#[test]
fn preferences_legacy_color_scheme_migrates_to_light_and_dark() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("prefs.json");

    std::fs::write(&path, r#"{"color_scheme": "Solarized Dark"}"#).unwrap();
    let loaded = preferences::load_from(&path);

    assert_eq!(loaded.light_color_scheme, "Solarized Dark");
    assert_eq!(loaded.dark_color_scheme, "Solarized Dark");
}

#[test]
fn preferences_unknown_fields_are_ignored() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("prefs.json");

    std::fs::write(&path, r#"{"font": "Mono 12", "unknown_field": true, "future_setting": 42}"#)
        .unwrap();
    let loaded = preferences::load_from(&path);
    assert_eq!(loaded.font, "Mono 12");
}

#[test]
fn preferences_input_sync_persists_in_session_state() {
    use rttx::runtime::WorkspaceRuntime;
    use rttx::workspace::{LayoutNode, WindowState, WorkspaceColor, WorkspaceState};

    let state = WindowState {
        workspaces: vec![WorkspaceState {
            uuid: "s1".into(),
            name: "Synced".into(),
            layout: LayoutNode::new_terminal(),
            terminal_recovery: std::collections::BTreeMap::default(),
            active_terminal_uuid: None,
            input_sync: false,
            runtime: WorkspaceRuntime::default(),
            color: WorkspaceColor::default(),
            zoomed_terminal_uuid: None,
            user_renamed: false,
        }],
        ..WindowState::default()
    };

    let json = serde_json::to_string(&state).unwrap();
    let loaded: WindowState = serde_json::from_str(&json).unwrap();
    assert!(!loaded.workspaces[0].input_sync);

    let mut state2 = state;
    state2.workspaces[0].input_sync = true;
    let json2 = serde_json::to_string(&state2).unwrap();
    let loaded2: WindowState = serde_json::from_str(&json2).unwrap();
    assert!(loaded2.workspaces[0].input_sync);
}

#[test]
fn preferences_backward_compat_missing_input_sync() {
    // Old session state JSON without input_sync or active_terminal_uuid should default safely.
    let json = r#"{
        "workspaces": [{
            "uuid": "s1",
            "name": "Test",
            "layout": {"Terminal": {"uuid": "t1", "profile": null, "cwd": null, "custom_title": null}}
        }],
        "active_workspace_index": 0,
        "width": 800,
        "height": 600,
        "is_maximized": false
    }"#;

    let state: rttx::workspace::WindowState = serde_json::from_str(json).unwrap();
    assert!(!state.workspaces[0].input_sync);
    assert!(state.workspaces[0].active_terminal_uuid.is_none());
}

#[test]
fn custom_title_persists_in_layout() {
    use rttx::runtime::WorkspaceRuntime;
    use rttx::workspace::{LayoutNode, WindowState, WorkspaceColor, WorkspaceState};

    let state = WindowState {
        workspaces: vec![WorkspaceState {
            uuid: "s1".into(),
            name: "Dev".into(),
            layout: LayoutNode::Terminal {
                uuid: "t1".into(),
                profile: None,
                cwd: Some("/home/user".into()),
                custom_title: Some("my editor".into()),
            },
            terminal_recovery: std::collections::BTreeMap::default(),
            active_terminal_uuid: None,
            input_sync: false,
            runtime: WorkspaceRuntime::default(),
            color: WorkspaceColor::default(),
            zoomed_terminal_uuid: None,
            user_renamed: false,
        }],
        ..WindowState::default()
    };

    let json = serde_json::to_string(&state).unwrap();
    let loaded: WindowState = serde_json::from_str(&json).unwrap();
    if let LayoutNode::Terminal { custom_title, .. } = &loaded.workspaces[0].layout {
        assert_eq!(custom_title.as_deref(), Some("my editor"));
    } else {
        panic!("Expected Terminal node");
    }
}

#[test]
fn custom_title_backward_compat_null() {
    // Old JSON without custom_title should deserialize as None
    let json = r#"{
        "workspaces": [{
            "uuid": "s1",
            "name": "Test",
            "layout": {"Terminal": {"uuid": "t1", "profile": null, "cwd": null, "custom_title": null}}
        }],
        "active_workspace_index": 0,
        "width": 800,
        "height": 600,
        "is_maximized": false
    }"#;
    let state: rttx::workspace::WindowState = serde_json::from_str(json).unwrap();
    if let rttx::workspace::LayoutNode::Terminal { custom_title, .. } = &state.workspaces[0].layout
    {
        assert_eq!(*custom_title, None);
    }
}

#[test]
fn pane_navigation_keys_persists_across_save_load() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("prefs.json");

    let prefs = Preferences {
        pane_navigation_keys: PaneNavigationKeys::CtrlShiftArrow,
        ..Default::default()
    };
    preferences::save_to(&prefs, &path).unwrap();
    let loaded = preferences::load_from(&path);
    assert_eq!(loaded.pane_navigation_keys, PaneNavigationKeys::CtrlShiftArrow);

    // Verify backward compatibility: old JSON without the field defaults to AltArrow.
    std::fs::write(&path, r#"{"font": "Mono 12"}"#).unwrap();
    let loaded = preferences::load_from(&path);
    assert_eq!(loaded.pane_navigation_keys, PaneNavigationKeys::AltArrow);
}

#[test]
fn paste_guard_defaults_to_enabled_with_1k_threshold() {
    let prefs = Preferences::default();
    assert!(prefs.paste_guard);
    assert_eq!(prefs.paste_guard_threshold, 1024);
}

#[test]
fn paste_guard_roundtrips() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("prefs.json");

    let prefs =
        Preferences { paste_guard: false, paste_guard_threshold: 4096, ..Default::default() };
    preferences::save_to(&prefs, &path).unwrap();
    let loaded = preferences::load_from(&path);
    assert!(!loaded.paste_guard);
    assert_eq!(loaded.paste_guard_threshold, 4096);
}

#[test]
fn paste_guard_backward_compat_missing_fields() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("prefs.json");
    std::fs::write(&path, r#"{"font": "Mono 12"}"#).unwrap();
    let loaded = preferences::load_from(&path);
    assert!(loaded.paste_guard);
    assert_eq!(loaded.paste_guard_threshold, 1024);
}

#[test]
fn trim_trailing_whitespace_on_copy_defaults_to_false() {
    let prefs = Preferences::default();
    assert!(!prefs.trim_trailing_whitespace_on_copy);
}

#[test]
fn trim_trailing_whitespace_on_copy_roundtrips() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("prefs.json");

    let prefs = Preferences { trim_trailing_whitespace_on_copy: true, ..Default::default() };
    preferences::save_to(&prefs, &path).unwrap();
    let loaded = preferences::load_from(&path);
    assert!(loaded.trim_trailing_whitespace_on_copy);
}

#[test]
fn trim_trailing_whitespace_on_copy_backward_compat_missing_field() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("prefs.json");
    std::fs::write(&path, r#"{"font": "Mono 12"}"#).unwrap();
    let loaded = preferences::load_from(&path);
    assert!(!loaded.trim_trailing_whitespace_on_copy);
}

#[test]
fn keyboard_shortcuts_defaults_to_empty() {
    let prefs = Preferences::default();
    assert!(prefs.keyboard_shortcuts.is_empty());
}

#[test]
fn keyboard_shortcuts_roundtrips() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("prefs.json");

    let mut shortcuts = BTreeMap::new();
    shortcuts.insert("close-terminal".into(), vec!["<Ctrl>q".into()]);
    shortcuts.insert("fullscreen".into(), vec![]);

    let prefs = Preferences { keyboard_shortcuts: shortcuts, ..Default::default() };
    preferences::save_to(&prefs, &path).unwrap();
    let loaded = preferences::load_from(&path);
    assert_eq!(loaded.keyboard_shortcuts["close-terminal"], vec!["<Ctrl>q"]);
    assert!(loaded.keyboard_shortcuts["fullscreen"].is_empty());
}

#[test]
fn keyboard_shortcuts_backward_compat_missing_field() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("prefs.json");
    std::fs::write(&path, r#"{"font": "Mono 12"}"#).unwrap();
    let loaded = preferences::load_from(&path);
    assert!(loaded.keyboard_shortcuts.is_empty());
}

#[test]
fn keyboard_shortcuts_migration_from_ctrl_shift_arrow() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("prefs.json");
    std::fs::write(&path, r#"{"pane_navigation_keys": "ctrl-shift-arrow"}"#).unwrap();
    let loaded = preferences::load_from(&path);
    assert_eq!(loaded.keyboard_shortcuts["navigate-left"], vec!["<Ctrl><Shift>Left"]);
    assert_eq!(loaded.keyboard_shortcuts["navigate-right"], vec!["<Ctrl><Shift>Right"]);
    assert_eq!(loaded.keyboard_shortcuts["navigate-up"], vec!["<Ctrl><Shift>Up"]);
    assert_eq!(loaded.keyboard_shortcuts["navigate-down"], vec!["<Ctrl><Shift>Down"]);
}

#[test]
fn keyboard_shortcuts_migration_noop_for_alt_arrow() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("prefs.json");
    std::fs::write(&path, r#"{"pane_navigation_keys": "alt-arrow"}"#).unwrap();
    let loaded = preferences::load_from(&path);
    assert!(!loaded.keyboard_shortcuts.contains_key("navigate-left"));
}

#[test]
fn keyboard_shortcuts_explicit_override_wins_over_migration() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("prefs.json");
    std::fs::write(
        &path,
        r#"{
            "pane_navigation_keys": "ctrl-shift-arrow",
            "keyboard_shortcuts": {"navigate-left": ["<Alt>h"]}
        }"#,
    )
    .unwrap();
    let loaded = preferences::load_from(&path);
    assert_eq!(loaded.keyboard_shortcuts["navigate-left"], vec!["<Alt>h"]);
    assert_eq!(loaded.keyboard_shortcuts["navigate-right"], vec!["<Ctrl><Shift>Right"]);
}
