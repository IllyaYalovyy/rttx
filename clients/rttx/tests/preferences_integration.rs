/// Integration tests for preferences persistence.
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
        default_session_folder: DefaultSessionFolder::Custom("/home/user/dev".into()),
        pane_navigation_keys: PaneNavigationKeys::AltArrow,
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
    use rttx::session::{LayoutNode, SessionColor, SessionMode, SessionState, WindowState};

    let state = WindowState {
        sessions: vec![SessionState {
            uuid: "s1".into(),
            name: "Synced".into(),
            layout: LayoutNode::new_terminal(),
            terminal_recovery: std::collections::BTreeMap::default(),
            active_terminal_uuid: None,
            input_sync: false,
            mode: SessionMode::default(),
            runtime: WorkspaceRuntime::default(),
            color: SessionColor::default(),
            zoomed_terminal_uuid: None,
            user_renamed: false,
        }],
        ..WindowState::default()
    };

    let json = serde_json::to_string(&state).unwrap();
    let loaded: WindowState = serde_json::from_str(&json).unwrap();
    assert!(!loaded.sessions[0].input_sync);

    let mut state2 = state;
    state2.sessions[0].input_sync = true;
    let json2 = serde_json::to_string(&state2).unwrap();
    let loaded2: WindowState = serde_json::from_str(&json2).unwrap();
    assert!(loaded2.sessions[0].input_sync);
}

#[test]
fn preferences_backward_compat_missing_input_sync() {
    // Old session state JSON without input_sync or active_terminal_uuid should default safely.
    let json = r#"{
        "sessions": [{
            "uuid": "s1",
            "name": "Test",
            "layout": {"Terminal": {"uuid": "t1", "profile": null, "cwd": null, "custom_title": null}}
        }],
        "active_session_index": 0,
        "width": 800,
        "height": 600,
        "is_maximized": false
    }"#;

    let state: rttx::session::WindowState = serde_json::from_str(json).unwrap();
    assert!(!state.sessions[0].input_sync);
    assert!(state.sessions[0].active_terminal_uuid.is_none());
}

#[test]
fn custom_title_persists_in_layout() {
    use rttx::runtime::WorkspaceRuntime;
    use rttx::session::{LayoutNode, SessionColor, SessionMode, SessionState, WindowState};

    let state = WindowState {
        sessions: vec![SessionState {
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
            mode: SessionMode::default(),
            runtime: WorkspaceRuntime::default(),
            color: SessionColor::default(),
            zoomed_terminal_uuid: None,
            user_renamed: false,
        }],
        ..WindowState::default()
    };

    let json = serde_json::to_string(&state).unwrap();
    let loaded: WindowState = serde_json::from_str(&json).unwrap();
    if let LayoutNode::Terminal { custom_title, .. } = &loaded.sessions[0].layout {
        assert_eq!(custom_title.as_deref(), Some("my editor"));
    } else {
        panic!("Expected Terminal node");
    }
}

#[test]
fn custom_title_backward_compat_null() {
    // Old JSON without custom_title should deserialize as None
    let json = r#"{
        "sessions": [{
            "uuid": "s1",
            "name": "Test",
            "layout": {"Terminal": {"uuid": "t1", "profile": null, "cwd": null, "custom_title": null}}
        }],
        "active_session_index": 0,
        "width": 800,
        "height": 600,
        "is_maximized": false
    }"#;
    let state: rttx::session::WindowState = serde_json::from_str(json).unwrap();
    if let rttx::session::LayoutNode::Terminal { custom_title, .. } = &state.sessions[0].layout {
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
