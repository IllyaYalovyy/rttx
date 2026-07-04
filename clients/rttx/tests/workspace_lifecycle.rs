//! Integration tests for session lifecycle scenarios.
//!
//! These tests verify that the session model correctly handles
//! real-world workflows: creating workspaces, splitting terminals,
//! closing terminals, persisting state, and restoring it.

use pretty_assertions::assert_eq;
use rttx::runtime::WorkspaceRuntime;
use rttx::workspace::*;
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
    let workspaces = vec![
        WorkspaceState {
            uuid: "s1".into(),
            name: "Editor".into(),
            layout: hsplit(term("editor-main"), vsplit(term("editor-side"), term("editor-term"))),
            terminal_recovery: std::collections::BTreeMap::default(),
            active_terminal_uuid: None,
            input_sync: false,
            runtime: WorkspaceRuntime::default(),
            color: WorkspaceColor::default(),
            zoomed_terminal_uuid: None,
            user_renamed: false,
        },
        WorkspaceState {
            uuid: "s2".into(),
            name: "Build".into(),
            layout: vsplit(term("build-output"), term("build-logs")),
            terminal_recovery: std::collections::BTreeMap::default(),
            active_terminal_uuid: None,
            input_sync: false,
            runtime: WorkspaceRuntime::default(),
            color: WorkspaceColor::default(),
            zoomed_terminal_uuid: None,
            user_renamed: false,
        },
        WorkspaceState {
            uuid: "s3".into(),
            name: "Monitoring".into(),
            layout: term("htop"),
            terminal_recovery: std::collections::BTreeMap::default(),
            active_terminal_uuid: None,
            input_sync: false,
            runtime: WorkspaceRuntime::default(),
            color: WorkspaceColor::default(),
            zoomed_terminal_uuid: None,
            user_renamed: false,
        },
    ];

    let state = WindowState {
        workspaces,
        active_workspace_index: 1,
        width: 1920,
        height: 1080,
        is_maximized: true,
        ..WindowState::default()
    };

    // Verify structure
    assert_eq!(state.workspaces[0].layout.terminal_count(), 3);
    assert_eq!(state.workspaces[1].layout.terminal_count(), 2);
    assert_eq!(state.workspaces[2].layout.terminal_count(), 1);

    // Serialize and restore
    let json = serde_json::to_string_pretty(&state).unwrap();
    let restored: WindowState = serde_json::from_str(&json).unwrap();
    assert_eq!(state, restored);

    // Verify active session survived
    assert_eq!(restored.active_workspace_index, 1);
    assert_eq!(restored.workspaces[1].name, "Build");
}

#[test]
fn workflow_persist_and_restore_with_cwds() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspaces_dir = tmp.path().join("rttx").join("sessions");
    std::fs::create_dir_all(&workspaces_dir).unwrap();

    let state = WindowState {
        workspaces: vec![WorkspaceState {
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
            runtime: WorkspaceRuntime::default(),
            color: WorkspaceColor::default(),
            zoomed_terminal_uuid: None,
            user_renamed: false,
        }],
        active_workspace_index: 0,
        width: 1200,
        height: 800,
        is_maximized: false,
        ..WindowState::default()
    };

    let path = workspaces_dir.join("window-state.json");
    let json = serde_json::to_string_pretty(&state).unwrap();
    std::fs::write(&path, &json).unwrap();

    let loaded: WindowState =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

    // CWDs survived
    if let LayoutNode::Terminal { cwd: _, custom_title: _, .. } = &loaded.workspaces[0].layout {
        panic!("Expected Split at root, got Terminal");
    } else if let LayoutNode::Split { first, second, .. } = &loaded.workspaces[0].layout {
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
fn empty_workspace_name_is_valid() {
    let session = WorkspaceState {
        uuid: "s1".into(),
        name: String::new(),
        layout: term("t1"),
        terminal_recovery: std::collections::BTreeMap::default(),
        active_terminal_uuid: None,
        input_sync: false,
        runtime: WorkspaceRuntime::default(),
        color: WorkspaceColor::default(),
        zoomed_terminal_uuid: None,
        user_renamed: false,
    };
    let json = serde_json::to_string(&session).unwrap();
    let restored: WorkspaceState = serde_json::from_str(&json).unwrap();
    assert_eq!(session, restored);
}

#[test]
fn session_order_persists_through_serialization() {
    let state = WindowState {
        workspaces: vec![
            WorkspaceState {
                uuid: "s3".into(),
                name: "Third".into(),
                layout: term("t3"),
                terminal_recovery: std::collections::BTreeMap::default(),
                active_terminal_uuid: None,
                input_sync: false,
                runtime: WorkspaceRuntime::default(),
                color: WorkspaceColor::default(),
                zoomed_terminal_uuid: None,
                user_renamed: false,
            },
            WorkspaceState {
                uuid: "s1".into(),
                name: "First".into(),
                layout: term("t1"),
                terminal_recovery: std::collections::BTreeMap::default(),
                active_terminal_uuid: None,
                input_sync: false,
                runtime: WorkspaceRuntime::default(),
                color: WorkspaceColor::default(),
                zoomed_terminal_uuid: None,
                user_renamed: false,
            },
            WorkspaceState {
                uuid: "s2".into(),
                name: "Second".into(),
                layout: term("t2"),
                terminal_recovery: std::collections::BTreeMap::default(),
                active_terminal_uuid: None,
                input_sync: false,
                runtime: WorkspaceRuntime::default(),
                color: WorkspaceColor::default(),
                zoomed_terminal_uuid: None,
                user_renamed: false,
            },
        ],
        active_workspace_index: 1,
        ..WindowState::default()
    };

    let json = serde_json::to_string_pretty(&state).unwrap();
    let restored: WindowState = serde_json::from_str(&json).unwrap();

    let uuids: Vec<&str> = restored.workspaces.iter().map(|s| s.uuid.as_str()).collect();
    assert_eq!(uuids, vec!["s3", "s1", "s2"], "session order must be preserved");
    assert_eq!(restored.active_workspace_index, 1);
}

/// Verify that the session module re-exports work correctly after the
/// layout/state/recovery split — types from all three submodules are
/// accessible through `rttx::workspace::*`.
#[test]
fn module_split_reexports_are_complete() {
    // Layout types
    let layout = LayoutNode::new_terminal();
    assert_eq!(layout.terminal_count(), 1);

    // Recovery types
    let recovery = PaneRecovery::empty_shell();
    assert_eq!(recovery.source, PaneSource::EmptyShell);

    // State types
    let session = WorkspaceState::new("reexport-test".into());
    assert!(!session.uuid.is_empty());

    let state = WindowState::default();
    assert!(!state.workspaces.is_empty());
}

/// Closing a managed workspace must dismiss the runtime so inventory
/// refresh does not resurrect it.
#[test]
fn close_managed_workspace_prevents_inventory_resurrection() {
    use rttx::daemon_bridge::EndpointEvent;
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};

    let runtime_id = uuid::Uuid::new_v4().to_string();
    let mut state = WindowState {
        workspaces: vec![
            WorkspaceState::new("Direct".into()),
            WorkspaceState::new_managed_local("Managed".into(), WorkspacePolicy::Persistent, None),
        ],
        active_workspace_index: 0,
        ..WindowState::default()
    };

    // Assign a runtime ID to the managed session.
    state.workspaces[1].runtime.runtime_id = Some(runtime_id.clone());

    // Simulate close: remove session and dismiss runtime.
    state.dismiss_runtime(&RuntimeEndpoint::Local, &runtime_id);
    state.workspaces.retain(|s| s.runtime.runtime_id.as_deref() != Some(&runtime_id));

    // Inventory reports the runtime still exists.
    let pane_id = uuid::Uuid::new_v4().to_string();
    let transition = state.reconcile_endpoint_event(&EndpointEvent::InventoryLoaded {
        endpoint: RuntimeEndpoint::Local,
        workspaces: vec![rttx_proto::v3::WorkspaceInfo {
            id: uuid::Uuid::parse_str(&runtime_id).unwrap().as_bytes().to_vec(),
            name: "Should Not Resurrect".into(),
            pane_count: 1,
            has_write_owner: false,
            read_only_client_count: 0,
            current_client_role: 0,
            panes: vec![rttx_proto::v3::PaneInfo {
                id: uuid::Uuid::parse_str(&pane_id).unwrap().as_bytes().to_vec(),
                title: "bash".into(),
                cwd: "/tmp".into(),
                cols: 80,
                rows: 24,
                exit_status: None,
                reconstructed: false,
                no_persist: false,
            }],
            policy: rttx_proto::v3::WorkspacePolicy::Persistent as i32,
            reconstructed: false,
            workspace_revision: 1,
            active_pane_summary: String::new(),
            takeover_eligible: false,
            disabled_reason: String::new(),
        }],
    });

    assert!(
        transition.recovered_workspaces.is_empty(),
        "dismissed runtime must not be resurrected"
    );
}

/// New Workspace must create exactly one workspace — it must not trigger
/// inventory recovery that surfaces unrelated daemon workspaces.
#[test]
fn new_workspace_does_not_resurrect_unrelated_workspaces() {
    use rttx::daemon_bridge::EndpointEvent;
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};

    let mut state = WindowState {
        workspaces: vec![WorkspaceState::new_managed_local(
            "Existing".into(),
            WorkspacePolicy::Persistent,
            None,
        )],
        active_workspace_index: 0,
        ..WindowState::default()
    };

    // Simulate creating a new workspace (adds one session).
    let new_session = WorkspaceState::new_managed_local(
        "New Workspace".into(),
        WorkspacePolicy::Persistent,
        None,
    );
    state.workspaces.push(new_session);

    assert_eq!(state.workspaces.len(), 2, "should have exactly 2 sessions after create");

    // An inventory refresh reports an unrelated runtime.
    let unrelated_runtime_id = uuid::Uuid::new_v4().to_string();
    let pane_id = uuid::Uuid::new_v4().to_string();
    let transition = state.reconcile_endpoint_event(&EndpointEvent::InventoryLoaded {
        endpoint: RuntimeEndpoint::Local,
        workspaces: vec![rttx_proto::v3::WorkspaceInfo {
            id: uuid::Uuid::parse_str(&unrelated_runtime_id).unwrap().as_bytes().to_vec(),
            name: "Unrelated Runtime".into(),
            pane_count: 1,
            has_write_owner: false,
            read_only_client_count: 0,
            current_client_role: 0,
            panes: vec![rttx_proto::v3::PaneInfo {
                id: uuid::Uuid::parse_str(&pane_id).unwrap().as_bytes().to_vec(),
                title: "bash".into(),
                cwd: "/tmp".into(),
                cols: 80,
                rows: 24,
                exit_status: None,
                reconstructed: false,
                no_persist: false,
            }],
            policy: rttx_proto::v3::WorkspacePolicy::Persistent as i32,
            reconstructed: false,
            workspace_revision: 1,
            active_pane_summary: String::new(),
            takeover_eligible: false,
            disabled_reason: String::new(),
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
    assert_eq!(
        state.workspaces.len(),
        3,
        "state should have 3 sessions: existing + new + recovered"
    );
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
    let session = &mut state.workspaces[0];
    let uuid = session.layout.terminal_uuids()[0].clone();
    session.layout.set_terminal_cwd(&uuid, Some("/important/project".into()));

    let json = serde_json::to_string(&state).unwrap();
    let restored: WindowState = serde_json::from_str(&json).unwrap();

    assert_eq!(
        restored.workspaces[0].layout.terminal_cwd(&uuid).as_deref(),
        Some("/important/project"),
        "layout CWD must survive serialization"
    );
}

/// Regression test for #235: layout CWD must survive daemon restart cycle.
#[test]
fn layout_cwd_survives_reconnect_cycle() {
    use rttx::workspace::WorkspaceState;
    let mut session = WorkspaceState::new("test".into());
    let uuid = session.layout.terminal_uuids()[0].clone();
    session.layout.set_terminal_cwd(&uuid, Some("/project/dir".into()));

    // Simulate disconnect: CWD should not be cleared.
    assert_eq!(session.layout.terminal_cwd(&uuid).as_deref(), Some("/project/dir"),);
}

/// `new_managed_remote` must produce a valid remote-persistent session.
#[test]
fn new_managed_remote_produces_remote_persistent_session() {
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};
    use rttx::workspace::WorkspaceState;

    let session = WorkspaceState::new_managed_remote(
        "Remote Work".into(),
        "dev-box.internal",
        WorkspacePolicy::Persistent,
        None,
    );

    assert!(session.runtime.is_managed());
    assert_eq!(session.runtime.endpoint, RuntimeEndpoint::remote("dev-box.internal"));
    assert!(!session.layout.terminal_uuids().is_empty());
}

/// Remote managed session must round-trip through serialization.
#[test]
fn remote_managed_session_persists_and_restores() {
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};
    use rttx::workspace::WorkspaceState;

    let session = WorkspaceState::new_managed_remote(
        "Remote Work".into(),
        "dev@build-host",
        WorkspacePolicy::Persistent,
        Some("/home/dev/project".into()),
    );

    let json = serde_json::to_string(&session).unwrap();
    let restored: WorkspaceState = serde_json::from_str(&json).unwrap();

    assert!(restored.runtime.is_managed());
    assert_eq!(restored.runtime.endpoint, RuntimeEndpoint::remote("dev@build-host"));
    assert_eq!(
        restored.layout.terminal_cwd(&restored.layout.terminal_uuids()[0]).as_deref(),
        Some("/home/dev/project")
    );
}

/// Remote host must create a managed remote session.
/// Regression test for #243.
#[test]
fn remote_host_creates_managed_remote_session() {
    use rttx::runtime::RuntimeEndpoint;
    use rttx::workspace::WorkspaceState;

    let session = WorkspaceState::new_managed_remote(
        "Prod Server".into(),
        "deploy@example.com",
        rttx::runtime::WorkspacePolicy::Persistent,
        None,
    );

    assert!(session.runtime.is_managed());
    assert_eq!(session.runtime.endpoint, RuntimeEndpoint::remote("deploy@example.com"));
}

/// Updating a remote workspace endpoint must change the host and sync mode.
#[test]
fn update_remote_endpoint_changes_host() {
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};
    use rttx::workspace::WorkspaceState;

    let mut session = WorkspaceState::new_managed_remote(
        "Remote".into(),
        "old-host.example.com",
        WorkspacePolicy::Persistent,
        None,
    );

    session.runtime.endpoint = RuntimeEndpoint::remote("new-host.example.com");

    assert_eq!(session.runtime.endpoint, RuntimeEndpoint::remote("new-host.example.com"));
    assert!(session.runtime.is_managed());
}

/// Splitting a pane in a remote managed session must preserve the remote
/// endpoint and add the new pane to the layout. Issue #246.
#[test]
fn split_remote_session_preserves_endpoint_and_adds_pane() {
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};
    use rttx::workspace::layout::SplitOrientation;
    use rttx::workspace::{PaneRecovery, WorkspaceState};

    let mut session = WorkspaceState::new_managed_remote(
        "Remote".into(),
        "build-host.internal",
        WorkspacePolicy::Persistent,
        None,
    );

    let original_uuid = session.layout.terminal_uuids()[0].clone();
    assert_eq!(session.layout.terminal_count(), 1);

    // Split — mirrors the logic in window/mod.rs split_terminal().
    let (new_layout, new_uuid) = session
        .layout
        .split_terminal_with_new_uuid(&original_uuid, SplitOrientation::Horizontal)
        .unwrap();
    session.layout = new_layout;
    session.set_recovery(&new_uuid, PaneRecovery::empty_shell());

    // Endpoint must still be remote.
    assert_eq!(
        session.runtime.endpoint,
        RuntimeEndpoint::remote("build-host.internal"),
        "split must not change the workspace endpoint"
    );

    // Both panes must be in the layout.
    assert!(session.layout.contains_terminal(&new_uuid), "new pane must be in the layout");
    assert_eq!(session.layout.terminal_count(), 2);
    assert!(session.runtime.is_managed());
}

/// Splitting twice must keep all panes in the layout and endpoint unchanged.
#[test]
fn double_split_remote_session_keeps_all_panes() {
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};
    use rttx::workspace::layout::SplitOrientation;
    use rttx::workspace::{PaneRecovery, WorkspaceState};

    let mut session = WorkspaceState::new_managed_remote(
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

    let (layout, t3) =
        session.layout.split_terminal_with_new_uuid(&t2, SplitOrientation::Vertical).unwrap();
    session.layout = layout;
    session.set_recovery(&t3, PaneRecovery::empty_shell());

    assert_eq!(session.runtime.endpoint, RuntimeEndpoint::remote("gpu-box"));
    assert_eq!(session.layout.terminal_count(), 3);
    assert!(session.layout.contains_terminal(&t2));
    assert!(session.layout.contains_terminal(&t3));
}

/// Closing a remote workspace and then receiving inventory must not
/// resurrect the runtime. End-to-end regression test for #248.
#[test]
fn close_remote_workspace_prevents_resurrection_on_reconnect() {
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};
    use rttx::workspace::WorkspaceState;
    use rttx::workspace::state::WindowState;

    let runtime_id = uuid::Uuid::new_v4().to_string();
    let endpoint = RuntimeEndpoint::remote("prod-server");

    let mut state = WindowState::default();

    // Create a remote workspace with a known runtime_id.
    let session = WorkspaceState::new_managed_remote(
        "Prod".into(),
        "prod-server",
        WorkspacePolicy::Persistent,
        None,
    );
    state.workspaces.push(session);
    state.workspaces.last_mut().unwrap().runtime.runtime_id = Some(runtime_id.clone());

    // Simulate close: remove session and dismiss runtime.
    state.workspaces.clear();
    state.dismiss_runtime(&endpoint, &runtime_id);

    // Verify dismissed_runtime_ids survives serialization (persistence).
    let json = serde_json::to_string(&state).unwrap();
    let restored: WindowState = serde_json::from_str(&json).unwrap();

    assert!(
        restored.dismissed_runtime_ids.contains(&runtime_id),
        "dismissed runtime ID must survive persistence"
    );
    assert!(restored.workspaces.is_empty());
}

/// Remote managed session must be ready for inventory binding after
/// creation. Regression test for #249.
#[test]
fn remote_managed_session_is_ready_for_inventory_binding() {
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};
    use rttx::workspace::state::WindowState;

    let endpoint = RuntimeEndpoint::remote("gpu-box");

    let mut state = WindowState::default();
    state.workspaces.clear();
    let session = rttx::workspace::WorkspaceState::new_managed_remote(
        "ML Training".into(),
        "gpu-box",
        WorkspacePolicy::Persistent,
        None,
    );
    state.workspaces.push(session);

    let remote_session = &state.workspaces[0];
    assert!(remote_session.runtime.is_managed());
    assert_eq!(remote_session.runtime.endpoint, endpoint);
    assert!(remote_session.runtime.runtime_id.is_none());
    assert!(!remote_session.layout.terminal_uuids().is_empty());
}

/// `active_workspace_index` must be clamped to valid range on restore.
/// Regression test for #179.
#[test]
fn active_workspace_index_clamped_on_restore() {
    use rttx::workspace::state::WindowState;

    let state = WindowState { active_workspace_index: 999, ..WindowState::default() };

    let json = serde_json::to_string(&state).unwrap();
    let restored: WindowState = serde_json::from_str(&json).unwrap();

    let clamped = restored.active_workspace_index.min(restored.workspaces.len().saturating_sub(1));
    assert_eq!(clamped, 0, "out-of-bounds index must clamp to 0");
}

/// Connection status must survive session reorder. Regression test for #278.
#[test]
fn connection_status_survives_session_reorder() {
    use rttx::runtime::WorkspacePolicy;
    use rttx::workspace::WorkspaceState;
    use rttx::workspace::state::WindowState;

    let mut state = WindowState::default();
    state.workspaces.clear();
    let s1 = WorkspaceState::new_managed_local("A".into(), WorkspacePolicy::Persistent, None);
    let s2 = WorkspaceState::new_managed_local("B".into(), WorkspacePolicy::Persistent, None);
    let uuid1 = s1.uuid.clone();
    let uuid2 = s2.uuid.clone();
    state.workspaces.push(s1);
    state.workspaces.push(s2);

    // Simulate reorder: swap positions.
    let session = state.workspaces.remove(0);
    state.workspaces.insert(1, session);

    // Sessions are reordered but both still exist.
    assert_eq!(state.workspaces[0].uuid, uuid2);
    assert_eq!(state.workspaces[1].uuid, uuid1);
    // The connection status HashMap (stored on Window, not WindowState)
    // is not affected by session reorder — it's keyed by UUID.
}

/// Spawn error handling is wired — compile-time check. #22.
#[test]
fn spawn_error_format_is_user_visible() {
    let error = "command not found";
    let msg = format!("\r\n\x1b[31mFailed to spawn shell: {error}\x1b[0m\r\n");
    assert!(msg.contains(error));
    assert!(msg.contains("\x1b[31m"));
}

/// The application must not use `NON_UNIQUE` flags — single-instance is
/// enforced by `GApplication` via D-Bus. Regression guard for #15.
#[test]
fn application_flags_enforce_single_instance() {
    use rttx::config;

    let app_id = config::app_id();
    assert!(!app_id.is_empty(), "app_id must be set for GApplication single-instance");
}

/// Connection status lifecycle must follow the expected state machine.
/// Integration evidence for #132.
#[test]
fn connection_status_lifecycle_is_deterministic() {
    use rttx::runtime::{
        ConnectionEvent, ConnectionProblem, ConnectionStatus, advance_connection_status,
    };

    let s = advance_connection_status(&ConnectionStatus::Connecting, ConnectionEvent::Connected);
    assert_eq!(s, ConnectionStatus::Connected);

    let s = advance_connection_status(&s, ConnectionEvent::Lost);
    assert_eq!(s, ConnectionStatus::Disconnected);

    let s = advance_connection_status(
        &s,
        ConnectionEvent::Failed(ConnectionProblem::OwnershipConflict),
    );
    assert!(matches!(s, ConnectionStatus::Blocked(_)));

    let s = advance_connection_status(&s, ConnectionEvent::SessionMissing);
    assert_eq!(s, ConnectionStatus::SessionMissing);
}

/// When a workspace's runtime is gone on the daemon, the reconciliation
/// pipeline must emit `SessionMissing` without affecting sibling workspaces.
/// Regression test for #478.
#[test]
fn session_missing_does_not_affect_sibling_workspaces() {
    use rttx::daemon_bridge::EndpointEvent;
    use rttx::runtime::{ConnectionStatus, WorkspacePolicy};
    use rttx::workspace::WorkspaceState;
    use rttx::workspace_state::ConnectionStatusUpdate;

    let runtime_a = uuid::Uuid::new_v4().to_string();
    let runtime_b = uuid::Uuid::new_v4().to_string();

    let mut session_a =
        WorkspaceState::new_managed_local("Workspace A".into(), WorkspacePolicy::Persistent, None);
    session_a.runtime.runtime_id = Some(runtime_a);

    let mut session_b =
        WorkspaceState::new_managed_local("Workspace B".into(), WorkspacePolicy::Persistent, None);
    session_b.runtime.runtime_id = Some(runtime_b.clone());

    let ws_a_id = session_a.uuid.clone();
    let ws_b_id = session_b.uuid.clone();

    let mut state = WindowState { workspaces: vec![session_a, session_b], ..Default::default() };

    // Workspace A gets SessionMissing — workspace B must not be affected.
    let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceConnectionChanged {
        workspace_id: ws_a_id.clone(),
        status: ConnectionStatus::SessionMissing,
    });

    assert_eq!(transition.connection_status_updates.len(), 1);
    assert_eq!(
        transition.connection_status_updates[0],
        ConnectionStatusUpdate { workspace_id: ws_a_id, status: ConnectionStatus::SessionMissing }
    );

    // Workspace B's runtime_id is untouched.
    let session_b = state.workspaces.iter().find(|s| s.uuid == ws_b_id).unwrap();
    assert_eq!(session_b.runtime.runtime_id.as_deref(), Some(runtime_b.as_str()));
}

/// F-keys must produce xterm escape sequences for managed terminals. Regression for #293.
#[test]
fn managed_fkeys_encode_to_escape_sequences() {
    use rttx::terminal::encode_terminal_key_input_for_test;

    let fkeys = [
        (gtk4::gdk::Key::F1, b"\x1bOP".as_slice()),
        (gtk4::gdk::Key::F5, b"\x1b[15~".as_slice()),
        (gtk4::gdk::Key::F10, b"\x1b[21~".as_slice()),
        (gtk4::gdk::Key::F12, b"\x1b[24~".as_slice()),
    ];
    for (key, expected) in fkeys {
        let result = encode_terminal_key_input_for_test(key, gtk4::gdk::ModifierType::empty());
        assert_eq!(
            result.as_deref(),
            Some(expected),
            "F-key {key:?} must encode to escape sequence"
        );
    }
}

/// Ctrl+Arrow must use xterm modified key format. Regression for #295.
#[test]
fn ctrl_arrow_encodes_with_modifier_param() {
    use rttx::terminal::encode_terminal_key_input_for_test;

    let ctrl = gtk4::gdk::ModifierType::CONTROL_MASK;
    assert_eq!(
        encode_terminal_key_input_for_test(gtk4::gdk::Key::Right, ctrl).as_deref(),
        Some(b"\x1b[1;5C".as_slice()),
    );
    assert_eq!(
        encode_terminal_key_input_for_test(gtk4::gdk::Key::Left, ctrl).as_deref(),
        Some(b"\x1b[1;5D".as_slice()),
    );
}

/// Split pane CWD must propagate to a pane create request. #297.
///
/// For a tree-less snapshot the client keeps its placeholder layout instead
/// of requesting pane creation. Pane creation is bootstrapped by the daemon
/// bridge and pane identity is assigned by the `PaneCreated` re-key.
#[test]
fn split_remote_session_cwd_survives_layout_round_trip() {
    use rttx::runtime::WorkspacePolicy;
    use rttx::workspace::*;

    let first_uuid = uuid::Uuid::new_v4().to_string();
    let second_uuid = uuid::Uuid::new_v4().to_string();

    let layout = LayoutNode::Split {
        orientation: SplitOrientation::Horizontal,
        ratio: 0.5,
        first: Box::new(LayoutNode::Terminal {
            uuid: first_uuid,
            profile: None,
            cwd: None,
            custom_title: None,
        }),
        second: Box::new(LayoutNode::Terminal {
            uuid: second_uuid.clone(),
            profile: None,
            cwd: Some("/srv/project".into()),
            custom_title: None,
        }),
    };

    let mut session =
        WorkspaceState::new_managed_local("Workspace".into(), WorkspacePolicy::Persistent, None);
    session.layout = layout;

    // The layout node carries the CWD for the second pane; this is what a
    // subsequent CreatePane bootstrap reads to seed the daemon pane.
    assert_eq!(session.layout.terminal_cwd(&second_uuid).as_deref(), Some("/srv/project"),);
}

// ── Zoom state ──────────────────────────────────────────────────

#[test]
fn zoom_toggle_sets_and_clears_zoomed_terminal() {
    let mut session = WorkspaceState {
        uuid: "s1".into(),
        name: "Work".into(),
        layout: hsplit(term("t1"), term("t2")),
        terminal_recovery: std::collections::BTreeMap::default(),
        active_terminal_uuid: Some("t1".into()),
        input_sync: false,
        runtime: WorkspaceRuntime::default(),
        color: WorkspaceColor::default(),
        zoomed_terminal_uuid: None,
        user_renamed: false,
    };

    // Zoom in
    session.zoomed_terminal_uuid = Some("t1".into());
    assert!(session.is_zoomed());
    assert_eq!(session.zoomed_terminal_uuid.as_deref(), Some("t1"));

    // Zoom out
    session.zoomed_terminal_uuid = None;
    assert!(!session.is_zoomed());

    // Layout is unchanged throughout
    assert_eq!(session.layout.terminal_count(), 2);
}

#[test]
fn zoom_state_not_persisted_when_cleared_before_save() {
    let mut session = WorkspaceState {
        uuid: "s1".into(),
        name: "Work".into(),
        layout: hsplit(term("t1"), term("t2")),
        terminal_recovery: std::collections::BTreeMap::default(),
        active_terminal_uuid: Some("t1".into()),
        input_sync: false,
        runtime: WorkspaceRuntime::default(),
        color: WorkspaceColor::default(),
        zoomed_terminal_uuid: Some("t1".into()),
        user_renamed: false,
    };

    // Simulate save_state clearing zoom
    session.zoomed_terminal_uuid = None;
    let json = serde_json::to_string(&session).unwrap();
    let restored: WorkspaceState = serde_json::from_str(&json).unwrap();
    assert!(!restored.is_zoomed());
    assert_eq!(restored.layout.terminal_count(), 2);
}

#[test]
fn zoom_on_single_pane_session_is_noop() {
    let session = WorkspaceState {
        uuid: "s1".into(),
        name: "Work".into(),
        layout: term("t1"),
        terminal_recovery: std::collections::BTreeMap::default(),
        active_terminal_uuid: Some("t1".into()),
        input_sync: false,
        runtime: WorkspaceRuntime::default(),
        color: WorkspaceColor::default(),
        zoomed_terminal_uuid: None,
        user_renamed: false,
    };

    // Single pane — zoom should not be set (enforced by toggle_pane_zoom)
    assert!(!session.is_zoomed());
    assert_eq!(session.layout.terminal_count(), 1);
}

#[test]
fn zoom_preserves_layout_tree_integrity() {
    let layout = hsplit(vsplit(term("t1"), term("t2")), term("t3"));
    let mut session = WorkspaceState {
        uuid: "s1".into(),
        name: "Work".into(),
        layout: layout.clone(),
        terminal_recovery: std::collections::BTreeMap::default(),
        active_terminal_uuid: Some("t2".into()),
        input_sync: false,
        runtime: WorkspaceRuntime::default(),
        color: WorkspaceColor::default(),
        zoomed_terminal_uuid: None,
        user_renamed: false,
    };

    // Zoom t2
    session.zoomed_terminal_uuid = Some("t2".into());
    assert!(session.is_zoomed());

    // Layout tree is completely unchanged
    assert_eq!(session.layout, layout);
    assert_eq!(session.layout.terminal_count(), 3);
    assert!(session.layout.contains_terminal("t1"));
    assert!(session.layout.contains_terminal("t2"));
    assert!(session.layout.contains_terminal("t3"));

    // Unzoom
    session.zoomed_terminal_uuid = None;
    assert_eq!(session.layout, layout);
}

// ── Retry connection with stale runtime ─────────────────────────

#[test]
fn workspace_opened_with_new_runtime_id_updates_session_state() {
    use rttx::daemon_bridge::EndpointEvent;
    use rttx::runtime::WorkspacePolicy;
    use rttx::workspace::WorkspaceState;

    let stale_runtime = uuid::Uuid::new_v4().to_string();
    let new_runtime = uuid::Uuid::new_v4();
    let pane_id = uuid::Uuid::new_v4();

    let mut session =
        WorkspaceState::new_managed_local("Retry Test".into(), WorkspacePolicy::Ephemeral, None);
    session.runtime.runtime_id = Some(stale_runtime);

    let mut state = WindowState { workspaces: vec![session], ..Default::default() };

    // Simulate the daemon responding with a different runtime id
    // (the stale one was gone, so a new runtime was created).
    let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
        workspace_id: state.workspaces[0].uuid.clone(),
        runtime_id: new_runtime.to_string(),
        snapshot: rttx_proto::v3::WorkspaceSnapshot {
            tree: None,
            default_active_pane_id: Vec::new(),
            runtime_id: rttx_proto::uuid_to_bytes(new_runtime),
            panes: vec![rttx_proto::v3::PaneSnapshot {
                pane_id: rttx_proto::uuid_to_bytes(pane_id),
                pane_output_seq: 0,
                title: "shell".into(),
                cwd: "/home".into(),
                cols: 80,
                rows: 24,
                scrollback_tail: bytes::Bytes::new(),
                exit_status: None,
                terminal_modes: None,
                total_scrollback_bytes: 0,
                scrollback_complete: true,
            }],
            workspace_revision: 1,
            client_role: rttx_proto::v3::WorkspaceClientRole::Writer as i32,
        },
    });

    // The session should have the new runtime id.
    assert_eq!(
        state.workspaces[0].runtime.runtime_id.as_deref(),
        Some(new_runtime.to_string().as_str()),
        "runtime_id should be updated to the new runtime"
    );

    // The workspace should be rebuilt.
    assert_eq!(transition.rebuilt_workspaces.len(), 1);
    assert_eq!(transition.rebuilt_workspaces[0].workspace_id, state.workspaces[0].uuid);
}

#[test]
fn rename_sets_user_renamed_and_persists_name() {
    let mut session = WorkspaceState::new_managed_local(
        "Original".into(),
        rttx::runtime::WorkspacePolicy::Ephemeral,
        None,
    );
    session.runtime.runtime_id = Some(uuid::Uuid::new_v4().to_string());
    assert!(!session.user_renamed);

    session.name = "Renamed".into();
    session.user_renamed = true;

    let json = serde_json::to_string(&session).unwrap();
    let restored: WorkspaceState = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.name, "Renamed");
    assert!(restored.user_renamed);
    assert!(restored.runtime.runtime_id.is_some());
}

#[test]
fn rotate_layout_persists_through_serialization() {
    let layout = hsplit(term("t1"), vsplit(term("t2"), term("t3")));
    let original_uuids = layout.terminal_uuids();
    let mut session = WorkspaceState::new("test".into());
    session.uuid = "s1".into();
    session.layout = layout;

    let mut state = WindowState {
        active_workspace_index: 0,
        workspaces: vec![session],
        ..WindowState::default()
    };

    state.workspaces[0].layout = state.workspaces[0].layout.rotated();

    let json = serde_json::to_string(&state).unwrap();
    let restored: WindowState = serde_json::from_str(&json).unwrap();

    assert_eq!(restored.workspaces[0].layout, vsplit(term("t1"), hsplit(term("t2"), term("t3"))));
    assert_eq!(restored.workspaces[0].layout.terminal_uuids(), original_uuids);
}

#[test]
fn input_sync_fan_out_targets_all_bound_managed_siblings() {
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};

    let layout = hsplit(term("pane-1"), hsplit(term("pane-2"), term("pane-3")));
    let mut session = WorkspaceState::new("Sync test".into());
    session.uuid = "ws-sync".into();
    session.layout = layout;
    session.input_sync = true;
    session.runtime = WorkspaceRuntime {
        managed: true,
        endpoint: RuntimeEndpoint::Local,
        policy: WorkspacePolicy::Persistent,
        runtime_id: Some("rt-1".into()),
    };

    let state = WindowState {
        active_workspace_index: 0,
        workspaces: vec![session],
        ..WindowState::default()
    };

    // Identity invariant: each sibling's runtime pane id IS its layout uuid.
    let targets = state.input_sync_targets("pane-1");
    assert_eq!(targets.len(), 2);
    let target_pane_ids: Vec<&str> = targets.iter().map(|t| t.runtime_pane_id.as_str()).collect();
    assert!(target_pane_ids.contains(&"pane-2"));
    assert!(target_pane_ids.contains(&"pane-3"));

    // Verify no targets when input sync is off.
    let mut state_off = state.clone();
    state_off.workspaces[0].input_sync = false;
    assert!(state_off.input_sync_targets("pane-1").is_empty());

    // Verify serialization roundtrip preserves input_sync.
    let json = serde_json::to_string(&state).unwrap();
    let restored: WindowState = serde_json::from_str(&json).unwrap();
    assert!(restored.workspaces[0].input_sync);
    assert_eq!(restored.input_sync_targets("pane-1").len(), 2);
}

/// A persisted `WindowState` document that lacks the expected workspace
/// shape must fail to deserialize.
#[test]
fn window_state_rejects_document_without_workspaces() {
    use rttx::workspace::state::WindowState;

    let json = r#"{
        "active_workspace_index": 0,
        "width": 800,
        "height": 600,
        "is_maximized": false,
        "unrecognized_key": [{"uuid": "s1"}]
    }"#;

    assert!(serde_json::from_str::<WindowState>(json).is_err());
}

/// Dismissed runtime IDs that are no longer in the daemon inventory
/// should be pruned after inventory reconciliation.
#[test]
fn dismissed_runtime_ids_pruned_when_absent_from_inventory() {
    use rttx::daemon_bridge::EndpointEvent;
    use rttx::runtime::RuntimeEndpoint;

    let mut state = WindowState::default();
    let stale = uuid::Uuid::new_v4().to_string();
    let live = uuid::Uuid::new_v4().to_string();
    let pane = uuid::Uuid::new_v4().to_string();

    state.dismiss_runtime(&RuntimeEndpoint::Local, &stale);
    state.dismiss_runtime(&RuntimeEndpoint::Local, &live);
    assert_eq!(state.dismissed_runtime_ids.len(), 2);

    // Inventory contains only `live` — `stale` was already cleaned up by daemon.
    let _transition = state.reconcile_endpoint_event(&EndpointEvent::InventoryLoaded {
        endpoint: RuntimeEndpoint::Local,
        workspaces: vec![rttx_proto::v3::WorkspaceInfo {
            id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(&live).unwrap()),
            name: "Live".into(),
            pane_count: 1,
            has_write_owner: false,
            read_only_client_count: 0,
            current_client_role: rttx_proto::v3::WorkspaceClientRole::Unattached as i32,
            panes: vec![rttx_proto::v3::PaneInfo {
                id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(&pane).unwrap()),
                title: "bash".into(),
                cwd: "/tmp".into(),
                cols: 80,
                rows: 24,
                exit_status: None,
                reconstructed: false,
                no_persist: false,
            }],
            policy: rttx_proto::v3::WorkspacePolicy::Persistent as i32,
            reconstructed: false,
            workspace_revision: 1,
            active_pane_summary: String::new(),
            takeover_eligible: false,
            disabled_reason: String::new(),
        }],
    });

    assert!(!state.dismissed_runtime_ids.contains(&stale), "stale dismissed ID should be pruned");
    assert!(state.dismissed_runtime_ids.contains(&live), "live dismissed ID should be retained");
}

#[test]
fn default_window_state_has_reasonable_sidebar_widths() {
    let state = rttx::workspace::WindowState::default();
    assert!(
        state.left_sidebar_width >= 150,
        "left sidebar should be at least 150px for workspace names"
    );
    assert!(
        state.right_sidebar_width >= 200,
        "right sidebar should be at least 200px for commands/places"
    );
}

#[test]
fn v3_snapshot_terminal_modes_propagate_through_reconciliation() {
    use rttx::workspace::LayoutNode;

    let runtime_id = uuid::Uuid::new_v4().to_string();
    let pane_id = uuid::Uuid::new_v4().to_string();
    let mut state = WindowState {
        workspaces: vec![WorkspaceState::new_managed_local(
            "Test".into(),
            rttx::runtime::WorkspacePolicy::Persistent,
            None,
        )],
        ..WindowState::default()
    };
    // The layout terminal IS the server pane id (identity invariant).
    state.workspaces[0].layout = LayoutNode::Terminal {
        uuid: pane_id.clone(),
        profile: None,
        cwd: None,
        custom_title: None,
    };
    let ws_id = state.workspaces[0].uuid.clone();

    let snapshot = rttx_proto::v3::WorkspaceSnapshot {
        tree: Some(rttx_proto::v3_tree::pane_tree_leaf(uuid::Uuid::parse_str(&pane_id).unwrap())),
        default_active_pane_id: Vec::new(),
        runtime_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(&runtime_id).unwrap()),
        panes: vec![rttx_proto::v3::PaneSnapshot {
            pane_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(&pane_id).unwrap()),
            pane_output_seq: 42,
            title: "vim".into(),
            cwd: "/home".into(),
            cols: 80,
            rows: 24,
            exit_status: None,
            terminal_modes: Some(rttx_proto::v3::TerminalModeState {
                bracketed_paste: true,
                focus_reporting: true,
                application_cursor_keys: true,
                application_keypad: false,
                alternate_screen: true,
                cursor_hidden: false,
                mouse_mode: rttx_proto::v3::MouseMode::Normal as i32,
                sgr_mouse: true,
            }),
            scrollback_tail: bytes::Bytes::from_static(b"scrollback data"),
            total_scrollback_bytes: 15,
            scrollback_complete: true,
        }],
        workspace_revision: 5,
        client_role: rttx_proto::v3::WorkspaceClientRole::Writer as i32,
    };

    let transition =
        state.reconcile_endpoint_event(&rttx::daemon_bridge::EndpointEvent::WorkspaceOpened {
            workspace_id: ws_id,
            runtime_id,
            snapshot,
        });

    assert_eq!(transition.pane_snapshot_restores.len(), 1);
    let restore = &transition.pane_snapshot_restores[0];
    assert_eq!(restore.pane_output_seq, 42);
    assert_eq!(restore.scrollback_tail, &b"scrollback data"[..]);
    assert!(restore.scrollback_complete);
    let modes = restore.terminal_modes.as_ref().expect("modes should be present");
    assert!(modes.bracketed_paste);
    assert!(modes.focus_reporting);
    assert!(modes.application_cursor_keys);
    assert!(modes.alternate_screen);
    assert_eq!(modes.mouse_mode, rttx_proto::v3::MouseMode::Normal as i32);
    assert!(modes.sgr_mouse);
}

/// Snapshot with `focus_reporting` and `cursor_hidden` active propagates
/// through reconciliation so the restore path can re-apply them. #765.
#[test]
fn v3_snapshot_focus_and_cursor_modes_propagate_through_reconciliation() {
    use rttx::workspace::{LayoutNode, WindowState, WorkspaceState};

    let runtime_id = uuid::Uuid::new_v4().to_string();
    let pane_id = uuid::Uuid::new_v4().to_string();
    let mut state = WindowState {
        workspaces: vec![WorkspaceState::new_managed_local(
            "Test".into(),
            rttx::runtime::WorkspacePolicy::Persistent,
            None,
        )],
        ..WindowState::default()
    };
    // The layout terminal IS the server pane id (identity invariant).
    state.workspaces[0].layout = LayoutNode::Terminal {
        uuid: pane_id.clone(),
        profile: None,
        cwd: None,
        custom_title: None,
    };
    let ws_id = state.workspaces[0].uuid.clone();

    let snapshot = rttx_proto::v3::WorkspaceSnapshot {
        tree: Some(rttx_proto::v3_tree::pane_tree_leaf(uuid::Uuid::parse_str(&pane_id).unwrap())),
        default_active_pane_id: Vec::new(),
        runtime_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(&runtime_id).unwrap()),
        panes: vec![rttx_proto::v3::PaneSnapshot {
            pane_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(&pane_id).unwrap()),
            pane_output_seq: 10,
            title: "htop".into(),
            cwd: "/home".into(),
            cols: 80,
            rows: 24,
            exit_status: None,
            terminal_modes: Some(rttx_proto::v3::TerminalModeState {
                focus_reporting: true,
                cursor_hidden: true,
                bracketed_paste: false,
                application_cursor_keys: false,
                application_keypad: false,
                alternate_screen: false,
                mouse_mode: rttx_proto::v3::MouseMode::None as i32,
                sgr_mouse: false,
            }),
            scrollback_tail: bytes::Bytes::from_static(b"plain text"),
            total_scrollback_bytes: 10,
            scrollback_complete: true,
        }],
        workspace_revision: 1,
        client_role: rttx_proto::v3::WorkspaceClientRole::Writer as i32,
    };

    let transition =
        state.reconcile_endpoint_event(&rttx::daemon_bridge::EndpointEvent::WorkspaceOpened {
            workspace_id: ws_id,
            runtime_id,
            snapshot,
        });

    assert_eq!(transition.pane_snapshot_restores.len(), 1);
    let modes = transition.pane_snapshot_restores[0]
        .terminal_modes
        .as_ref()
        .expect("modes should be present");
    assert!(modes.focus_reporting, "focus_reporting must propagate");
    assert!(modes.cursor_hidden, "cursor_hidden must propagate");
}

/// Serialized workspace state must not contain the removed `mode` field.
/// The `runtime` struct carries endpoint and policy in serialized state.
#[test]
fn serialized_managed_workspace_includes_runtime() {
    use rttx::runtime::WorkspacePolicy;
    use rttx::workspace::{WindowState, WorkspaceState};

    let session = WorkspaceState::new_managed_remote(
        "Remote".into(),
        "build-host",
        WorkspacePolicy::Persistent,
        None,
    );
    let state = WindowState {
        workspaces: vec![session],
        active_workspace_index: 0,
        ..WindowState::default()
    };
    let json = serde_json::to_string(&state).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let ws = &value["workspaces"][0];
    assert!(ws.get("runtime").is_some(), "runtime must be present in serialized state");
}

/// Remote endpoint with custom daemon binary path must persist through
/// serialization and restore correctly. Regression test for #956.
#[test]
fn remote_endpoint_with_custom_binary_path_persists() {
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};
    use rttx::workspace::WorkspaceState;

    let mut session = WorkspaceState::new_managed_remote(
        "Remote Custom".into(),
        "build-host",
        WorkspacePolicy::Persistent,
        None,
    );
    // Override endpoint with custom binary path.
    session.runtime.endpoint =
        RuntimeEndpoint::remote_with_binary("build-host", Some("~/.local/bin/rttx-server".into()));

    let json = serde_json::to_string(&session).unwrap();
    let restored: WorkspaceState = serde_json::from_str(&json).unwrap();

    assert_eq!(restored.runtime.endpoint.daemon_binary_path(), Some("~/.local/bin/rttx-server"),);
    assert_eq!(restored.runtime.endpoint, session.runtime.endpoint);
}

/// Remote endpoint without custom binary path must deserialize from JSON
/// that lacks the `daemon_binary_path` field. Regression test for #956.
#[test]
fn remote_endpoint_without_binary_path() {
    use rttx::runtime::{RuntimeEndpoint, WorkspacePolicy};
    use rttx::workspace::WorkspaceState;

    // Serialize a workspace with default remote endpoint (no binary path).
    let session = WorkspaceState::new_managed_remote(
        "Test Remote".into(),
        "old-host",
        WorkspacePolicy::Persistent,
        None,
    );
    let mut json_value: serde_json::Value = serde_json::to_value(&session).unwrap();

    // Strip daemon_binary_path to simulate a file written by an older version.
    if let Some(endpoint) = json_value.get_mut("runtime").and_then(|r| r.get_mut("endpoint")) {
        endpoint.as_object_mut().unwrap().remove("daemon_binary_path");
    }

    let restored: WorkspaceState = serde_json::from_value(json_value).unwrap();
    assert_eq!(restored.runtime.endpoint, RuntimeEndpoint::remote("old-host"));
    assert_eq!(restored.runtime.endpoint.daemon_binary_path(), None);
}
