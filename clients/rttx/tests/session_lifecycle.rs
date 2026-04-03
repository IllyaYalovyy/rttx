//! Integration tests for session lifecycle scenarios.
//!
//! These tests verify that the session model correctly handles
//! real-world workflows: creating sessions, splitting terminals,
//! closing terminals, persisting state, and restoring it.

use pretty_assertions::assert_eq;
use rttx::runtime::WorkspaceRuntime;
use rttx::session::*;

// ── Helpers (can't use test_helpers from lib, so inline) ─────────

fn term(id: &str) -> LayoutNode {
    LayoutNode::Terminal { uuid: id.to_string(), profile: None, cwd: None, custom_title: None }
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

// ── Workflow: typical user session ───────────────────────────────

#[test]
fn workflow_split_split_close_close() {
    // User starts with one terminal
    let mut layout = term("t1");
    assert_eq!(layout.terminal_count(), 1);

    // Split right → t1 | t2
    layout = layout.split_terminal("t1", SplitOrientation::Horizontal).unwrap();
    let t2 = layout.terminal_uuids().into_iter().find(|u| u != "t1").unwrap();
    assert_eq!(layout.terminal_count(), 2);

    // Split t1 down → (t1 / t3) | t2
    layout = layout.split_terminal("t1", SplitOrientation::Vertical).unwrap();
    let t3 = layout.terminal_uuids().into_iter().find(|u| u != "t1" && u != &t2).unwrap();
    assert_eq!(layout.terminal_count(), 3);

    // Close t3 → t1 | t2
    layout = layout.remove_terminal(&t3).unwrap();
    assert_eq!(layout.terminal_count(), 2);
    assert!(!layout.contains_terminal(&t3));

    // Close t2 → t1
    layout = layout.remove_terminal(&t2).unwrap();
    assert_eq!(layout.terminal_count(), 1);
    assert!(layout.contains_terminal("t1"));
}

#[test]
fn workflow_multi_session_state() {
    // Simulate a window with 3 sessions
    let sessions = vec![
        SessionState {
            uuid: "s1".into(),
            name: "Editor".into(),
            layout: hsplit(term("editor-main"), vsplit(term("editor-side"), term("editor-term"))),
            terminal_recovery: std::collections::BTreeMap::default(),
            active_terminal_uuid: None,
            input_sync: false,
            mode: SessionMode::default(),
            runtime: WorkspaceRuntime::default(),
        },
        SessionState {
            uuid: "s2".into(),
            name: "Build".into(),
            layout: vsplit(term("build-output"), term("build-logs")),
            terminal_recovery: std::collections::BTreeMap::default(),
            active_terminal_uuid: None,
            input_sync: false,
            mode: SessionMode::default(),
            runtime: WorkspaceRuntime::default(),
        },
        SessionState {
            uuid: "s3".into(),
            name: "Monitoring".into(),
            layout: term("htop"),
            terminal_recovery: std::collections::BTreeMap::default(),
            active_terminal_uuid: None,
            input_sync: false,
            mode: SessionMode::default(),
            runtime: WorkspaceRuntime::default(),
        },
    ];

    let state = WindowState {
        sessions,
        active_session_index: 1,
        width: 1920,
        height: 1080,
        is_maximized: true,
        ..WindowState::default()
    };

    // Verify structure
    assert_eq!(state.sessions[0].layout.terminal_count(), 3);
    assert_eq!(state.sessions[1].layout.terminal_count(), 2);
    assert_eq!(state.sessions[2].layout.terminal_count(), 1);

    // Serialize and restore
    let json = serde_json::to_string_pretty(&state).unwrap();
    let restored: WindowState = serde_json::from_str(&json).unwrap();
    assert_eq!(state, restored);

    // Verify active session survived
    assert_eq!(restored.active_session_index, 1);
    assert_eq!(restored.sessions[1].name, "Build");
}

#[test]
fn workflow_persist_and_restore_with_cwds() {
    let tmp = tempfile::TempDir::new().unwrap();
    let sessions_dir = tmp.path().join("rttx").join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();

    let state = WindowState {
        sessions: vec![SessionState {
            uuid: "s1".into(),
            name: "Dev".into(),
            layout: hsplit(
                LayoutNode::Terminal {
                    uuid: "t1".into(),
                    profile: Some("default".into()),
                    cwd: Some("/home/user/project/src".into()),
                    custom_title: Some("vim".into()),
                },
                LayoutNode::Terminal {
                    uuid: "t2".into(),
                    profile: Some("default".into()),
                    cwd: Some("/home/user/project".into()),
                    custom_title: Some("cargo watch".into()),
                },
            ),
            terminal_recovery: std::collections::BTreeMap::default(),
            active_terminal_uuid: None,
            input_sync: false,
            mode: SessionMode::default(),
            runtime: WorkspaceRuntime::default(),
        }],
        active_session_index: 0,
        width: 1200,
        height: 800,
        is_maximized: false,
        ..WindowState::default()
    };

    let path = sessions_dir.join("window-state.json");
    let json = serde_json::to_string_pretty(&state).unwrap();
    std::fs::write(&path, &json).unwrap();

    let loaded: WindowState =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

    // CWDs survived
    if let LayoutNode::Terminal { cwd: _, custom_title: _, .. } = &loaded.sessions[0].layout {
        panic!("Expected Split at root, got Terminal");
    } else if let LayoutNode::Split { first, second, .. } = &loaded.sessions[0].layout {
        if let LayoutNode::Terminal { cwd, custom_title, .. } = first.as_ref() {
            assert_eq!(cwd.as_deref(), Some("/home/user/project/src"));
            assert_eq!(custom_title.as_deref(), Some("vim"));
        }
        if let LayoutNode::Terminal { cwd, custom_title, .. } = second.as_ref() {
            assert_eq!(cwd.as_deref(), Some("/home/user/project"));
            assert_eq!(custom_title.as_deref(), Some("cargo watch"));
        }
    }
}

// ── Edge cases ───────────────────────────────────────────────────

#[test]
fn remove_all_terminals_one_by_one() {
    let mut layout = hsplit(hsplit(term("a"), term("b")), hsplit(term("c"), term("d")));
    assert_eq!(layout.terminal_count(), 4);

    for target in ["a", "b", "c"] {
        layout = layout.remove_terminal(target).unwrap();
    }
    assert_eq!(layout.terminal_count(), 1);
    assert!(layout.contains_terminal("d"));

    // Last one returns None
    assert!(layout.remove_terminal("d").is_none());
}

#[test]
fn split_same_terminal_multiple_times() {
    let mut layout = term("t1");

    for _ in 0..5 {
        layout = layout.split_terminal("t1", SplitOrientation::Horizontal).unwrap();
    }

    assert_eq!(layout.terminal_count(), 6);
    assert!(layout.contains_terminal("t1"));
}

#[test]
fn deeply_nested_layout_serializes() {
    // Build a 10-deep chain
    let mut layout = term("t0");
    for i in 1..10 {
        layout = hsplit(layout, term(&format!("t{i}")));
    }

    assert_eq!(layout.terminal_count(), 10);

    let json = serde_json::to_string(&layout).unwrap();
    let restored: LayoutNode = serde_json::from_str(&json).unwrap();
    assert_eq!(layout, restored);
}

#[test]
fn empty_session_name_is_valid() {
    let session = SessionState {
        uuid: "s1".into(),
        name: String::new(),
        layout: term("t1"),
        terminal_recovery: std::collections::BTreeMap::default(),
        active_terminal_uuid: None,
        input_sync: false,
        mode: SessionMode::default(),
        runtime: WorkspaceRuntime::default(),
    };
    let json = serde_json::to_string(&session).unwrap();
    let restored: SessionState = serde_json::from_str(&json).unwrap();
    assert_eq!(session, restored);
}

#[test]
fn session_order_persists_through_serialization() {
    let state = WindowState {
        sessions: vec![
            SessionState {
                uuid: "s3".into(),
                name: "Third".into(),
                layout: term("t3"),
                terminal_recovery: std::collections::BTreeMap::default(),
                active_terminal_uuid: None,
                input_sync: false,
                mode: SessionMode::default(),
                runtime: WorkspaceRuntime::default(),
            },
            SessionState {
                uuid: "s1".into(),
                name: "First".into(),
                layout: term("t1"),
                terminal_recovery: std::collections::BTreeMap::default(),
                active_terminal_uuid: None,
                input_sync: false,
                mode: SessionMode::default(),
                runtime: WorkspaceRuntime::default(),
            },
            SessionState {
                uuid: "s2".into(),
                name: "Second".into(),
                layout: term("t2"),
                terminal_recovery: std::collections::BTreeMap::default(),
                active_terminal_uuid: None,
                input_sync: false,
                mode: SessionMode::default(),
                runtime: WorkspaceRuntime::default(),
            },
        ],
        active_session_index: 1,
        ..WindowState::default()
    };

    let json = serde_json::to_string_pretty(&state).unwrap();
    let restored: WindowState = serde_json::from_str(&json).unwrap();

    let uuids: Vec<&str> = restored.sessions.iter().map(|s| s.uuid.as_str()).collect();
    assert_eq!(uuids, vec!["s3", "s1", "s2"], "session order must be preserved");
    assert_eq!(restored.active_session_index, 1);
}

/// Verify that the session module re-exports work correctly after the
/// layout/state/recovery split — types from all three submodules are
/// accessible through `rttx::session::*`.
#[test]
fn module_split_reexports_are_complete() {
    // Layout types
    let layout = LayoutNode::new_terminal();
    assert_eq!(layout.terminal_count(), 1);

    // Recovery types
    let recovery = PaneRecovery::empty_shell();
    assert_eq!(recovery.source, PaneSource::EmptyShell);

    // State types
    let session = SessionState::new("reexport-test".into());
    assert_eq!(session.mode, SessionMode::Direct);

    let state = WindowState::default();
    assert!(!state.sessions.is_empty());
}

/// Closing a managed workspace must dismiss the runtime so inventory
/// refresh does not resurrect it.
#[test]
fn close_managed_workspace_prevents_inventory_resurrection() {
    use rttx::daemon_bridge::EndpointEvent;
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};

    let runtime_id = uuid::Uuid::new_v4().to_string();
    let mut state = WindowState {
        sessions: vec![
            SessionState::new("Direct".into()),
            SessionState::new_managed_local("Managed".into(), WorkspacePolicy::Persistent, None),
        ],
        active_session_index: 0,
        ..WindowState::default()
    };

    // Assign a runtime ID to the managed session.
    state.sessions[1].runtime.runtime_id = Some(runtime_id.clone());

    // Simulate close: remove session and dismiss runtime.
    state.dismiss_runtime(&RuntimeEndpoint::Local, &runtime_id);
    state.sessions.retain(|s| s.runtime.runtime_id.as_deref() != Some(&runtime_id));

    // Inventory reports the runtime still exists.
    let pane_id = uuid::Uuid::new_v4().to_string();
    let transition = state.reconcile_endpoint_event(&EndpointEvent::InventoryLoaded {
        endpoint: RuntimeEndpoint::Local,
        sessions: vec![rttx_proto::proto::SessionInfo {
            id: uuid::Uuid::parse_str(&runtime_id).unwrap().as_bytes().to_vec(),
            name: "Should Not Resurrect".into(),
            pane_count: 1,
            has_attached_client: false,
            active_pane_id: None,
            panes: vec![rttx_proto::proto::PaneInfo {
                id: uuid::Uuid::parse_str(&pane_id).unwrap().as_bytes().to_vec(),
                title: "bash".into(),
                cwd: "/tmp".into(),
                cols: 80,
                rows: 24,
                exit_status: None,
                reconstructed: false,
            }],
            policy: rttx_proto::proto::RuntimePolicy::Persistent as i32,
            attached_client_count: 0,
            reconstructed: false,
            revision: 1,
            current_client_role: 0,
            has_write_owner: false,
            read_only_client_count: 0,
        }],
    });

    assert!(
        transition.recovered_workspaces.is_empty(),
        "dismissed runtime must not be resurrected"
    );
}
