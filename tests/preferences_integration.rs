/// Integration tests for preferences persistence.
use rttx::preferences::{self, Preferences};
use tempfile::TempDir;

#[test]
fn preferences_default_values_are_reasonable() {
    let prefs = Preferences::default();
    assert!(prefs.font.contains("12") || prefs.font.contains("Mono"));
    assert!(prefs.scrollback_lines >= 1000);
    assert!(prefs.background_opacity > 0.0);
    assert!(prefs.background_opacity <= 1.0);
}

#[test]
fn preferences_roundtrip_all_fields() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("prefs.json");

    let prefs = Preferences {
        font: "Fira Code 16".into(),
        color_scheme: "solarized-dark".into(),
        scrollback_lines: 50000,
        show_headerbar: false,
        scroll_on_keystroke: false,
        scroll_on_output: true,
        audible_bell: false,
        background_opacity: 0.85,
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
    assert_eq!(loaded.color_scheme, "default");
    assert_eq!(loaded.scrollback_lines, 10000);
    assert!(loaded.show_headerbar);
}

#[test]
fn preferences_unknown_fields_are_ignored() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("prefs.json");

    std::fs::write(
        &path,
        r#"{"font": "Mono 12", "unknown_field": true, "future_setting": 42}"#,
    )
    .unwrap();
    let loaded = preferences::load_from(&path);
    assert_eq!(loaded.font, "Mono 12");
}

#[test]
fn preferences_input_sync_persists_in_session_state() {
    use rttx::session::layout::{LayoutNode, SessionState, WindowState};

    let state = WindowState {
        sessions: vec![SessionState {
            uuid: "s1".into(),
            name: "Synced".into(),
            layout: LayoutNode::new_terminal(),
            input_sync: false,
        }],
        active_session_index: 0,
        width: 800,
        height: 600,
        is_maximized: false,
    };

    let json = serde_json::to_string(&state).unwrap();
    let loaded: WindowState = serde_json::from_str(&json).unwrap();
    assert!(!loaded.sessions[0].input_sync);

    let mut state2 = state.clone();
    state2.sessions[0].input_sync = true;
    let json2 = serde_json::to_string(&state2).unwrap();
    let loaded2: WindowState = serde_json::from_str(&json2).unwrap();
    assert!(loaded2.sessions[0].input_sync);
}

#[test]
fn preferences_backward_compat_missing_input_sync() {
    // Old session state JSON without input_sync field should default to false
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

    let state: rttx::session::layout::WindowState = serde_json::from_str(json).unwrap();
    assert!(!state.sessions[0].input_sync);
}

#[test]
fn custom_title_persists_in_layout() {
    use rttx::session::layout::{LayoutNode, SessionState, WindowState};

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
            input_sync: false,
        }],
        active_session_index: 0,
        width: 800,
        height: 600,
        is_maximized: false,
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
    let state: rttx::session::layout::WindowState = serde_json::from_str(json).unwrap();
    if let rttx::session::layout::LayoutNode::Terminal { custom_title, .. } =
        &state.sessions[0].layout
    {
        assert_eq!(*custom_title, None);
    }
}
