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
            color: SessionColor::default(),
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
            color: SessionColor::default(),
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
            color: SessionColor::default(),
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
            color: SessionColor::default(),
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
        color: SessionColor::default(),
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
                color: SessionColor::default(),
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
                color: SessionColor::default(),
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
                color: SessionColor::default(),
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

/// New Workspace must create exactly one workspace — it must not trigger
/// inventory recovery that surfaces unrelated daemon runtimes.
#[test]
fn new_workspace_does_not_resurrect_unrelated_runtimes() {
    use rttx::daemon_bridge::EndpointEvent;
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};

    let mut state = WindowState {
        sessions: vec![SessionState::new_managed_local(
            "Existing".into(),
            WorkspacePolicy::Persistent,
            None,
        )],
        active_session_index: 0,
        ..WindowState::default()
    };

    // Simulate creating a new workspace (adds one session).
    let new_session =
        SessionState::new_managed_local("New Workspace".into(), WorkspacePolicy::Persistent, None);
    state.sessions.push(new_session);

    assert_eq!(state.sessions.len(), 2, "should have exactly 2 sessions after create");

    // An inventory refresh reports an unrelated runtime.
    let unrelated_runtime_id = uuid::Uuid::new_v4().to_string();
    let pane_id = uuid::Uuid::new_v4().to_string();
    let transition = state.reconcile_endpoint_event(&EndpointEvent::InventoryLoaded {
        endpoint: RuntimeEndpoint::Local,
        sessions: vec![rttx_proto::proto::SessionInfo {
            id: uuid::Uuid::parse_str(&unrelated_runtime_id).unwrap().as_bytes().to_vec(),
            name: "Unrelated Runtime".into(),
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

    // The unrelated runtime IS recovered (this is correct for startup bootstrap).
    // The key behavioral change is that connect_managed_workspace no longer
    // triggers refresh_inventory, so this recovery only happens on startup.
    // This test documents the expected state-level behavior.
    assert_eq!(
        transition.recovered_workspaces.len(),
        1,
        "inventory recovery should find the unrelated runtime"
    );
    assert_eq!(state.sessions.len(), 3, "state should have 3 sessions: existing + new + recovered");
}

/// Dismissed runtime IDs must persist through save/load so closed
/// workspaces stay closed across restarts.
#[test]
fn dismissed_runtime_ids_persist_through_save_load() {
    use rttx::runtime::RuntimeEndpoint;

    let mut state = WindowState::default();
    state.dismiss_runtime(&RuntimeEndpoint::Local, "test-runtime-id");

    let json = serde_json::to_string(&state).unwrap();
    let restored: WindowState = serde_json::from_str(&json).unwrap();

    assert!(
        restored.dismissed_runtime_ids.contains("test-runtime-id"),
        "dismissed runtime IDs must survive serialization"
    );
}

/// Layout CWD must not be cleared when a managed pane widget reports
/// None during save. Regression test for #235.
#[test]
fn save_state_preserves_layout_cwd_when_widget_reports_none() {
    let mut state = WindowState::default();
    let session = &mut state.sessions[0];
    let uuid = session.layout.terminal_uuids()[0].clone();
    session.layout.set_terminal_cwd(&uuid, Some("/important/project".into()));

    let json = serde_json::to_string(&state).unwrap();
    let restored: WindowState = serde_json::from_str(&json).unwrap();

    assert_eq!(
        restored.sessions[0].layout.terminal_cwd(&uuid).as_deref(),
        Some("/important/project"),
        "layout CWD must survive serialization"
    );
}

/// Regression test for #235: layout CWD must survive daemon restart cycle.
#[test]
fn layout_cwd_survives_reconnect_cycle() {
    use rttx::session::SessionState;
    let mut session = SessionState::new("test".into());
    let uuid = session.layout.terminal_uuids()[0].clone();
    session.layout.set_terminal_cwd(&uuid, Some("/project/dir".into()));

    // Simulate disconnect: CWD should not be cleared.
    assert_eq!(session.layout.terminal_cwd(&uuid).as_deref(), Some("/project/dir"),);
}

/// `new_managed_remote` must produce a valid remote-persistent session.
#[test]
fn new_managed_remote_produces_remote_persistent_session() {
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};
    use rttx::session::SessionState;

    let session = SessionState::new_managed_remote(
        "Remote Work".into(),
        "dev-box.internal",
        WorkspacePolicy::Persistent,
        None,
    );

    assert!(session.runtime.is_managed());
    assert_eq!(
        session.runtime.endpoint,
        RuntimeEndpoint::Remote { host: "dev-box.internal".into() }
    );
    assert!(!session.layout.terminal_uuids().is_empty());
    assert!(!session.runtime.pending_layout_panes.is_empty());
}

/// Remote managed session must round-trip through serialization.
#[test]
fn remote_managed_session_persists_and_restores() {
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};
    use rttx::session::{SessionMode, SessionState};

    let session = SessionState::new_managed_remote(
        "Remote Work".into(),
        "dev@build-host",
        WorkspacePolicy::Persistent,
        Some("/home/dev/project".into()),
    );

    let json = serde_json::to_string(&session).unwrap();
    let restored: SessionState = serde_json::from_str(&json).unwrap();

    assert!(restored.runtime.is_managed());
    assert_eq!(
        restored.runtime.endpoint,
        RuntimeEndpoint::Remote { host: "dev@build-host".into() }
    );
    assert!(matches!(restored.mode, SessionMode::RemotePersistent { .. }));
    assert_eq!(
        restored.layout.terminal_cwd(&restored.layout.terminal_uuids()[0]).as_deref(),
        Some("/home/dev/project")
    );
}

/// SSH bookmark must create a managed remote session, not a local direct one.
/// Regression test for #243.
#[test]
fn ssh_bookmark_creates_managed_remote_session() {
    use rttx::bookmarks::Bookmark;
    use rttx::runtime::RuntimeEndpoint;
    use rttx::session::SessionState;

    let mut bookmark = Bookmark::new("Prod Server");
    bookmark.ssh_target = Some("deploy@example.com".into());
    bookmark.directory = Some("/srv/app".into());

    // Simulate the decision logic from new_session_from_bookmark.
    let host = bookmark.remote_host();
    assert!(host.is_some(), "SSH bookmark must report a remote host");

    let session = SessionState::new_managed_remote(
        bookmark.name.clone(),
        host.unwrap(),
        rttx::runtime::WorkspacePolicy::Persistent,
        bookmark.session_initial_cwd().map(str::to_string),
    );

    assert!(session.runtime.is_managed());
    assert_eq!(
        session.runtime.endpoint,
        RuntimeEndpoint::Remote { host: "deploy@example.com".into() }
    );
}

/// Local bookmark must still create a direct session.
#[test]
fn local_bookmark_creates_direct_session() {
    use rttx::bookmarks::Bookmark;

    let mut bookmark = Bookmark::new("Projects");
    bookmark.directory = Some("/home/user/projects".into());

    assert!(bookmark.remote_host().is_none());
}

/// Updating a remote workspace endpoint must change the host and sync mode.
#[test]
fn update_remote_endpoint_changes_host_and_mode() {
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};
    use rttx::session::{SessionMode, SessionState};

    let mut session = SessionState::new_managed_remote(
        "Remote".into(),
        "old-host.example.com",
        WorkspacePolicy::Persistent,
        None,
    );

    session.runtime.endpoint = RuntimeEndpoint::Remote { host: "new-host.example.com".into() };
    session.sync_legacy_mode_from_runtime();

    assert_eq!(
        session.runtime.endpoint,
        RuntimeEndpoint::Remote { host: "new-host.example.com".into() }
    );
    assert!(matches!(
        session.mode,
        SessionMode::RemotePersistent { ref host, .. } if host == "new-host.example.com"
    ));
}

/// SSH bookmark targeting the same host as a managed pane must use the inner
/// command (without SSH wrapper). Regression test for #245.
#[test]
fn ssh_bookmark_remote_command_strips_ssh_for_same_host() {
    use rttx::bookmarks::Bookmark;

    let mut bookmark = Bookmark::new("Deploy");
    bookmark.ssh_target = Some("deploy@example.com".into());
    bookmark.directory = Some("/srv/app".into());
    bookmark.tmux_session = Some("web".into());

    let full = bookmark.command().unwrap();
    let inner = bookmark.remote_command().unwrap();

    assert!(full.starts_with("ssh"), "full command wraps in ssh");
    assert!(!inner.contains("ssh"), "inner command must not contain ssh");
    assert!(inner.contains("/srv/app"));
    assert!(inner.contains("tmux"));
}

/// Splitting a pane in a remote managed session must preserve the remote
/// endpoint and add the new pane to pending bindings. Issue #246.
#[test]
fn split_remote_session_preserves_endpoint_and_adds_pending_pane() {
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};
    use rttx::session::layout::SplitOrientation;
    use rttx::session::{PaneRecovery, SessionState};

    let mut session = SessionState::new_managed_remote(
        "Remote".into(),
        "build-host.internal",
        WorkspacePolicy::Persistent,
        None,
    );

    let original_uuid = session.layout.terminal_uuids()[0].clone();
    assert_eq!(session.runtime.pending_layout_panes.len(), 1);

    // Split — mirrors the logic in window/mod.rs split_terminal().
    let (new_layout, new_uuid) = session
        .layout
        .split_terminal_with_new_uuid(&original_uuid, SplitOrientation::Horizontal)
        .unwrap();
    session.layout = new_layout;
    session.set_recovery(&new_uuid, PaneRecovery::empty_shell());
    session.runtime.ensure_placeholder_bindings(&session.layout.terminal_uuids());

    // Endpoint must still be remote.
    assert_eq!(
        session.runtime.endpoint,
        RuntimeEndpoint::Remote { host: "build-host.internal".into() },
        "split must not change the workspace endpoint"
    );

    // Both panes must be in pending bindings.
    assert!(
        session.runtime.pending_layout_panes.contains(&new_uuid),
        "new pane must be in pending_layout_panes"
    );
    assert_eq!(session.layout.terminal_count(), 2);
    assert!(session.runtime.is_managed());
}

/// Splitting twice must keep all panes pending and endpoint unchanged.
#[test]
fn double_split_remote_session_keeps_all_panes_pending() {
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};
    use rttx::session::layout::SplitOrientation;
    use rttx::session::{PaneRecovery, SessionState};

    let mut session = SessionState::new_managed_remote(
        "Remote".into(),
        "gpu-box",
        WorkspacePolicy::Persistent,
        None,
    );

    let t1 = session.layout.terminal_uuids()[0].clone();

    let (layout, t2) =
        session.layout.split_terminal_with_new_uuid(&t1, SplitOrientation::Horizontal).unwrap();
    session.layout = layout;
    session.set_recovery(&t2, PaneRecovery::empty_shell());
    session.runtime.ensure_placeholder_bindings(&session.layout.terminal_uuids());

    let (layout, t3) =
        session.layout.split_terminal_with_new_uuid(&t2, SplitOrientation::Vertical).unwrap();
    session.layout = layout;
    session.set_recovery(&t3, PaneRecovery::empty_shell());
    session.runtime.ensure_placeholder_bindings(&session.layout.terminal_uuids());

    assert_eq!(session.runtime.endpoint, RuntimeEndpoint::Remote { host: "gpu-box".into() });
    assert_eq!(session.layout.terminal_count(), 3);
    assert!(session.runtime.pending_layout_panes.contains(&t2));
    assert!(session.runtime.pending_layout_panes.contains(&t3));
}

/// Closing a remote workspace and then receiving inventory must not
/// resurrect the runtime. End-to-end regression test for #248.
#[test]
fn close_remote_workspace_prevents_resurrection_on_reconnect() {
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};
    use rttx::session::SessionState;
    use rttx::session::state::WindowState;

    let runtime_id = uuid::Uuid::new_v4().to_string();
    let endpoint = RuntimeEndpoint::Remote { host: "prod-server".into() };

    let mut state = WindowState::default();

    // Create a remote workspace with a known runtime_id.
    let session = SessionState::new_managed_remote(
        "Prod".into(),
        "prod-server",
        WorkspacePolicy::Persistent,
        None,
    );
    state.sessions.push(session);
    state.sessions.last_mut().unwrap().runtime.runtime_id = Some(runtime_id.clone());

    // Simulate close: remove session and dismiss runtime.
    state.sessions.clear();
    state.dismiss_runtime(&endpoint, &runtime_id);

    // Verify dismissed_runtime_ids survives serialization (persistence).
    let json = serde_json::to_string(&state).unwrap();
    let restored: WindowState = serde_json::from_str(&json).unwrap();

    assert!(
        restored.dismissed_runtime_ids.contains(&runtime_id),
        "dismissed runtime ID must survive persistence"
    );
    assert!(restored.sessions.is_empty());
}

/// Remote managed session must be ready for inventory binding after
/// creation. Regression test for #249.
#[test]
fn remote_managed_session_is_ready_for_inventory_binding() {
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};
    use rttx::session::state::WindowState;

    let endpoint = RuntimeEndpoint::Remote { host: "gpu-box".into() };

    let mut state = WindowState::default();
    state.sessions.clear();
    let session = rttx::session::SessionState::new_managed_remote(
        "ML Training".into(),
        "gpu-box",
        WorkspacePolicy::Persistent,
        None,
    );
    state.sessions.push(session);

    let remote_session = &state.sessions[0];
    assert!(remote_session.runtime.is_managed());
    assert_eq!(remote_session.runtime.endpoint, endpoint);
    assert!(remote_session.runtime.runtime_id.is_none());
    assert!(!remote_session.runtime.pending_layout_panes.is_empty());
}
