//! End-to-end session persistence tests.
//!
//! These tests exercise the full save → load cycle through serde and
//! file I/O, covering tolerance of older files that omit newer optional fields,
//! graceful recovery from corrupted state files, and correctness under
//! large/complex layouts.

use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy, WorkspaceRuntime};
use rttx::workspace::*;
use std::collections::BTreeMap;

// ── Inline helpers (test_helpers is cfg(test)-gated) ────────────

fn term(id: &str) -> LayoutNode {
    LayoutNode::Terminal { uuid: id.to_string(), profile: None, cwd: None, custom_title: None }
}

fn term_full(id: &str, cwd: &str, title: &str) -> LayoutNode {
    LayoutNode::Terminal {
        uuid: id.to_string(),
        profile: Some("default".into()),
        cwd: Some(cwd.into()),
        custom_title: Some(title.into()),
    }
}

fn hsplit(first: LayoutNode, second: LayoutNode) -> LayoutNode {
    LayoutNode::Split {
        orientation: SplitOrientation::Horizontal,
        ratio: 0.5,
        first: Box::new(first),
        second: Box::new(second),
    }
}

fn vsplit(first: LayoutNode, second: LayoutNode) -> LayoutNode {
    LayoutNode::Split {
        orientation: SplitOrientation::Vertical,
        ratio: 0.5,
        first: Box::new(first),
        second: Box::new(second),
    }
}

fn split_ratio(
    orientation: SplitOrientation,
    ratio: f64,
    first: LayoutNode,
    second: LayoutNode,
) -> LayoutNode {
    LayoutNode::Split { orientation, ratio, first: Box::new(first), second: Box::new(second) }
}

/// Save state to a temp directory and load it back via direct file I/O,
/// mirroring the real persistence path without depending on glib config
/// dir caching.
fn save_and_load(state: &WindowState) -> WindowState {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("workspaces.json");
    let json = serde_json::to_string_pretty(state).unwrap();
    std::fs::write(&path, &json).unwrap();
    let loaded_json = std::fs::read_to_string(&path).unwrap();
    let mut loaded: WindowState = serde_json::from_str(&loaded_json).unwrap();
    for session in &mut loaded.workspaces {
        session.normalize_active_terminal();
    }
    loaded
}

// ── Backward compatibility ──────────────────────────────────────

/// State saved before `left_sidebar_width` / `right_sidebar_width` existed
/// must load with sensible defaults.
#[test]
fn older_format_without_sidebar_widths_loads_with_defaults() {
    let json = r#"{
        "workspaces": [{
            "uuid": "s1",
            "name": "Test",
            "layout": {"Terminal": {"uuid": "t1"}}
        }],
        "active_workspace_index": 0,
        "width": 1024,
        "height": 768,
        "is_maximized": false
    }"#;

    let state: WindowState = serde_json::from_str(json).unwrap();
    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(state.workspaces[0].name, "Test");
    assert_eq!(state.left_sidebar_width, 220, "missing left_sidebar_width should default to 220");
    assert_eq!(state.right_sidebar_width, 320, "missing right_sidebar_width should default to 320");
}

/// State saved before `runtime`, `color`, `zoomed_terminal_uuid`, and
/// `user_renamed` fields existed must load with defaults for each.
#[test]
fn older_format_without_runtime_or_color_fields_loads_gracefully() {
    let json = r#"{
        "workspaces": [{
            "uuid": "s1",
            "name": "Session",
            "layout": {"Terminal": {"uuid": "t1"}},
            "terminal_recovery": {},
            "active_terminal_uuid": "t1",
            "input_sync": false,
            "unrecognized_field": "x"
        }],
        "active_workspace_index": 0,
        "width": 800,
        "height": 600,
        "is_maximized": false
    }"#;

    let state: WindowState = serde_json::from_str(json).unwrap();
    let session = &state.workspaces[0];
    assert_eq!(session.runtime, WorkspaceRuntime::default());
    assert_eq!(session.color, WorkspaceColor::Blue);
    assert!(session.zoomed_terminal_uuid.is_none());
    assert!(!session.user_renamed);
}

/// State saved before `dismissed_runtime_ids` existed must load with an
/// empty set.
#[test]
fn older_format_without_dismissed_runtime_ids_loads_empty() {
    let json = r#"{
        "workspaces": [{"uuid": "s1", "name": "W", "layout": {"Terminal": {"uuid": "t1"}}}],
        "active_workspace_index": 0,
        "width": 800,
        "height": 600,
        "is_maximized": false
    }"#;

    let state: WindowState = serde_json::from_str(json).unwrap();
    assert!(state.dismissed_runtime_ids.is_empty());
}

/// State containing an unknown field must deserialize without error,
/// silently ignoring the unrecognized field.
#[test]
fn unknown_field_is_silently_ignored() {
    let json = r#"{
        "uuid": "s1",
        "name": "Persistent Session",
        "layout": {"Terminal": {"uuid": "t1"}},
        "terminal_recovery": {},
        "active_terminal_uuid": "t1",
        "input_sync": false,
        "unrecognized_field": {"nested": "x"}
    }"#;

    let session: WorkspaceState = serde_json::from_str(json).unwrap();
    assert!(!session.uses_managed_runtime(), "an unknown field must not activate managed runtime");
    assert_eq!(session.uuid, "s1");
}

// ── Corrupted state ─────────────────────────────────────────────

/// Completely invalid JSON must fall back to default state via
/// `unwrap_or_default`.
#[test]
fn corrupted_json_falls_back_to_default() {
    let result: WindowState = serde_json::from_str("{{{{not json at all!!!!").unwrap_or_default();
    assert_eq!(result.workspaces.len(), 1, "corrupted JSON should produce default state");
    assert_eq!(result.workspaces[0].name, "Workspace 1");
}

/// Truncated JSON (e.g. from a crash during write) must fall back to
/// default state.
#[test]
fn truncated_json_falls_back_to_default() {
    let state = WindowState {
        workspaces: vec![WorkspaceState::new("First".into()), WorkspaceState::new("Second".into())],
        active_workspace_index: 1,
        ..WindowState::default()
    };
    let full_json = serde_json::to_string_pretty(&state).unwrap();
    let truncated = &full_json[..full_json.len() / 2];

    let loaded: WindowState = serde_json::from_str(truncated).unwrap_or_default();
    assert_eq!(loaded.workspaces.len(), 1, "truncated JSON should produce default state");
    assert_eq!(loaded.workspaces[0].name, "Workspace 1");
}

/// An empty string must fall back to default state.
#[test]
fn empty_string_falls_back_to_default() {
    let loaded: WindowState = serde_json::from_str("").unwrap_or_default();
    assert_eq!(loaded.workspaces.len(), 1);
}

/// JSON with unknown extra fields must still deserialize (forward compat).
#[test]
fn unknown_fields_are_ignored_gracefully() {
    let json = r#"{
        "workspaces": [{
            "uuid": "s1",
            "name": "Future",
            "layout": {"Terminal": {"uuid": "t1"}},
            "some_future_field": true,
            "another_new_thing": [1, 2, 3]
        }],
        "active_workspace_index": 0,
        "width": 800,
        "height": 600,
        "is_maximized": false,
        "future_window_field": "hello"
    }"#;

    let state: WindowState = serde_json::from_str(json).unwrap();
    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(state.workspaces[0].name, "Future");
}

// ── Large state ─────────────────────────────────────────────────

/// A workspace with 16 terminals must round-trip through file-based
/// persistence without data loss.
#[test]
fn large_layout_persists_and_restores_through_file() {
    let mut layout = term_full("t0", "/home/user/project-0", "vim");
    for i in 1..16 {
        layout = hsplit(
            layout,
            term_full(&format!("t{i}"), &format!("/home/user/project-{i}"), &format!("shell-{i}")),
        );
    }
    assert_eq!(layout.terminal_count(), 16);

    let state = WindowState {
        workspaces: vec![WorkspaceState {
            uuid: "large-session".into(),
            name: "Large Workspace".into(),
            layout,
            terminal_recovery: BTreeMap::default(),
            active_terminal_uuid: Some("t7".into()),
            input_sync: true,
            runtime: WorkspaceRuntime::default(),
            color: WorkspaceColor::Teal,
            zoomed_terminal_uuid: None,
            user_renamed: true,
        }],
        active_workspace_index: 0,
        width: 2560,
        height: 1440,
        is_maximized: true,
        ..WindowState::default()
    };

    let loaded = save_and_load(&state);

    assert_eq!(loaded.workspaces.len(), 1);
    assert_eq!(loaded.workspaces[0].layout.terminal_count(), 16);
    assert_eq!(loaded.workspaces[0].name, "Large Workspace");
    assert_eq!(loaded.workspaces[0].active_terminal_uuid.as_deref(), Some("t7"));
    assert!(loaded.workspaces[0].input_sync);
    assert_eq!(loaded.workspaces[0].color, WorkspaceColor::Teal);
    assert!(loaded.workspaces[0].user_renamed);
    assert!(loaded.is_maximized);

    for i in 0..16 {
        let uuid = format!("t{i}");
        assert_eq!(
            loaded.workspaces[0].layout.terminal_cwd(&uuid).as_deref(),
            Some(format!("/home/user/project-{i}").as_str()),
            "CWD for {uuid} must survive persistence"
        );
    }
}

/// Multiple workspaces with mixed configurations must all survive the
/// file-based save/load cycle.
#[test]
fn multi_workspace_mixed_config_persists_through_file() {
    let workspaces = vec![
        WorkspaceState {
            uuid: "ws-direct".into(),
            name: "Editor".into(),
            layout: hsplit(
                term_full("t1", "/home/user/src", "nvim"),
                vsplit(
                    term_full("t2", "/home/user/src", "cargo watch"),
                    term_full("t3", "/home/user/logs", "tail -f"),
                ),
            ),
            terminal_recovery: BTreeMap::default(),
            active_terminal_uuid: Some("t2".into()),
            input_sync: false,
            runtime: WorkspaceRuntime::default(),
            color: WorkspaceColor::Green,
            zoomed_terminal_uuid: None,
            user_renamed: true,
        },
        {
            let mut s = WorkspaceState::new_managed_local(
                "Build".into(),
                WorkspacePolicy::Persistent,
                Some("/home/user/build".into()),
            );
            s.uuid = "ws-managed".into();
            s.color = WorkspaceColor::Red;
            s
        },
        WorkspaceState {
            uuid: "ws-simple".into(),
            name: "Monitoring".into(),
            layout: term("t-mon"),
            terminal_recovery: BTreeMap::default(),
            active_terminal_uuid: Some("t-mon".into()),
            input_sync: false,
            runtime: WorkspaceRuntime::default(),
            color: WorkspaceColor::Purple,
            zoomed_terminal_uuid: None,
            user_renamed: false,
        },
    ];

    let state = WindowState {
        workspaces,
        active_workspace_index: 1,
        width: 1920,
        height: 1080,
        is_maximized: false,
        left_sidebar_width: 250,
        right_sidebar_width: 400,
        ..WindowState::default()
    };

    let loaded = save_and_load(&state);

    assert_eq!(loaded.workspaces.len(), 3);
    assert_eq!(loaded.active_workspace_index, 1);
    assert_eq!(loaded.left_sidebar_width, 250);
    assert_eq!(loaded.right_sidebar_width, 400);

    // Workspace order preserved.
    assert_eq!(loaded.workspaces[0].uuid, "ws-direct");
    assert_eq!(loaded.workspaces[1].uuid, "ws-managed");
    assert_eq!(loaded.workspaces[2].uuid, "ws-simple");

    // Names preserved.
    assert_eq!(loaded.workspaces[0].name, "Editor");
    assert_eq!(loaded.workspaces[1].name, "Build");
    assert_eq!(loaded.workspaces[2].name, "Monitoring");

    // Layout structure preserved.
    assert_eq!(loaded.workspaces[0].layout.terminal_count(), 3);
    assert_eq!(loaded.workspaces[2].layout.terminal_count(), 1);

    // Colors preserved.
    assert_eq!(loaded.workspaces[0].color, WorkspaceColor::Green);
    assert_eq!(loaded.workspaces[1].color, WorkspaceColor::Red);
    assert_eq!(loaded.workspaces[2].color, WorkspaceColor::Purple);

    // Active terminal preserved.
    assert_eq!(loaded.workspaces[0].active_terminal_uuid.as_deref(), Some("t2"));

    // Managed runtime metadata preserved.
    assert!(loaded.workspaces[1].runtime.is_managed());
    assert_eq!(loaded.workspaces[1].runtime.endpoint, RuntimeEndpoint::Local);
    assert_eq!(loaded.workspaces[1].runtime.policy, WorkspacePolicy::Persistent);
}

// ── Full round-trip through file persistence ────────────────────

/// Build state programmatically → save to file → load from file → verify
/// every field matches.
#[test]
fn full_roundtrip_through_file_persistence() {
    let mut dismissed = std::collections::BTreeSet::new();
    dismissed.insert("old-runtime-1".to_string());
    dismissed.insert("old-runtime-2".to_string());

    let state = WindowState {
        workspaces: vec![
            WorkspaceState {
                uuid: "ws-1".into(),
                name: "Development".into(),
                layout: split_ratio(
                    SplitOrientation::Horizontal,
                    0.35,
                    term_full("t1", "/home/user/rttx", "nvim"),
                    term_full("t2", "/home/user/rttx", "cargo test"),
                ),
                terminal_recovery: BTreeMap::default(),
                active_terminal_uuid: Some("t2".into()),
                input_sync: true,
                runtime: WorkspaceRuntime::default(),
                color: WorkspaceColor::Orange,
                zoomed_terminal_uuid: None,
                user_renamed: true,
            },
            WorkspaceState {
                uuid: "ws-2".into(),
                name: "Session 2".into(),
                layout: term("t3"),
                terminal_recovery: BTreeMap::default(),
                active_terminal_uuid: Some("t3".into()),
                input_sync: false,
                runtime: WorkspaceRuntime::default(),
                color: WorkspaceColor::Blue,
                zoomed_terminal_uuid: None,
                user_renamed: false,
            },
        ],
        active_workspace_index: 0,
        width: 1600,
        height: 900,
        is_maximized: false,
        left_sidebar_width: 200,
        right_sidebar_width: 350,
        dismissed_runtime_ids: dismissed,
        pane_reverse_index: std::collections::HashMap::new(),
    };

    let loaded = save_and_load(&state);

    // Window geometry.
    assert_eq!(loaded.width, 1600);
    assert_eq!(loaded.height, 900);
    assert!(!loaded.is_maximized);
    assert_eq!(loaded.left_sidebar_width, 200);
    assert_eq!(loaded.right_sidebar_width, 350);
    assert_eq!(loaded.active_workspace_index, 0);

    // Dismissed runtime IDs.
    assert!(loaded.dismissed_runtime_ids.contains("old-runtime-1"));
    assert!(loaded.dismissed_runtime_ids.contains("old-runtime-2"));

    // Session 1 details.
    let s1 = &loaded.workspaces[0];
    assert_eq!(s1.uuid, "ws-1");
    assert_eq!(s1.name, "Development");
    assert_eq!(s1.active_terminal_uuid.as_deref(), Some("t2"));
    assert!(s1.input_sync);
    assert_eq!(s1.color, WorkspaceColor::Orange);
    assert!(s1.user_renamed);
    assert_eq!(s1.layout.terminal_count(), 2);

    // Split ratio preserved.
    if let LayoutNode::Split { ratio, .. } = &s1.layout {
        assert!((*ratio - 0.35).abs() < 0.001, "split ratio should be preserved, got {ratio}");
    } else {
        panic!("expected split layout");
    }

    // CWDs and titles preserved.
    assert_eq!(s1.layout.terminal_cwd("t1").as_deref(), Some("/home/user/rttx"));
    assert_eq!(s1.layout.terminal_custom_title("t1").as_deref(), Some("nvim"));
    assert_eq!(s1.layout.terminal_custom_title("t2").as_deref(), Some("cargo test"));

    // Session 2 details.
    let s2 = &loaded.workspaces[1];
    assert_eq!(s2.uuid, "ws-2");
    assert_eq!(s2.name, "Session 2");
    assert!(!s2.input_sync);
    assert_eq!(s2.color, WorkspaceColor::Blue);
}

/// A managed workspace round-trips cleanly through the current format,
/// preserving its managed runtime.
#[test]
fn managed_workspace_state_round_trips_cleanly() {
    let session = WorkspaceState::new_managed_local(
        "Managed".into(),
        WorkspacePolicy::Persistent,
        Some("/home/user".into()),
    );
    let json = serde_json::to_string(&session).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(value.get("runtime").is_some(), "runtime must be present after roundtrip");

    let restored: WorkspaceState = serde_json::from_str(&json).unwrap();
    assert!(restored.uses_managed_runtime());
    assert_eq!(restored.runtime.endpoint, RuntimeEndpoint::Local);
}
