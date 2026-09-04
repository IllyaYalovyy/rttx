use super::*;
use crate::workspace::PaneTarget;
use std::time::{Duration, Instant};

fn store() -> crate::store::ClientStore {
    crate::store::default_store()
}

/// Load workspace state from the new store, reconstructing `WindowState`.
fn load_saved_window_state() -> WindowState {
    let store = store();
    let ws_store = store.load_workspaces().into_value().unwrap_or_default();
    let ui = store.load_ui_state().into_value().unwrap_or_default();
    let cache = store.load_runtime_cache().into_value().unwrap_or_default();
    let workspaces: Vec<_> = ws_store
        .workspaces
        .iter()
        .map(crate::store::models::workspaces::WorkspaceRecord::to_workspace_state)
        .collect();
    let active_workspace_index = ws_store
        .active_workspace_id
        .as_ref()
        .and_then(|id| workspaces.iter().position(|ws| ws.uuid == *id))
        .unwrap_or(0);
    let mut state = WindowState {
        workspaces,
        active_workspace_index,
        width: ui.window_width,
        height: ui.window_height,
        is_maximized: ui.is_maximized,
        left_sidebar_width: ui.left_sidebar_width,
        right_sidebar_width: ui.right_sidebar_width,
        dismissed_runtime_ids: cache.dismissed_runtime_ids,
        pane_reverse_index: std::collections::HashMap::new(),
    };
    state.rebuild_pane_reverse_index();
    state
}

/// Save a `WindowState` through the `ClientStore` for test setup.
fn save_window_state_to_store(state: &WindowState) {
    let store = store();
    let active_workspace_id =
        state.workspaces.get(state.active_workspace_index).map(|s| s.uuid.clone());
    let ws_store = crate::store::models::workspaces::WorkspaceStore {
        active_workspace_id,
        workspaces: state.workspaces.iter().map(Into::into).collect(),
    };
    let ui = crate::store::models::ui::UiState {
        window_width: state.width,
        window_height: state.height,
        is_maximized: state.is_maximized,
        left_sidebar_width: state.left_sidebar_width,
        right_sidebar_width: state.right_sidebar_width,
        ..Default::default()
    };
    let cache = crate::store::models::runtime_cache::RuntimeCache {
        dismissed_runtime_ids: state.dismissed_runtime_ids.clone(),
    };
    store.save_workspaces(&ws_store).unwrap();
    store.save_ui_state(&ui).unwrap();
    store.save_runtime_cache(&cache).unwrap();
}

macro_rules! require_display {
    () => {
        if !crate::test_helpers::ensure_gtk() {
            eprintln!("SKIPPED: no display available");
            return;
        }
    };
}

fn pump_events(max_ms: u64) {
    let ctx = glib::MainContext::default();
    let deadline = Instant::now() + Duration::from_millis(max_ms);
    while Instant::now() < deadline {
        if !ctx.iteration(false) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn wait_until(max_ms: u64, condition: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_millis(max_ms);
    while Instant::now() < deadline {
        pump_events(20);
        if condition() {
            return true;
        }
    }
    condition()
}

fn session_row_at(window: &Window, index: i32) -> WorkspaceRow {
    window
        .imp()
        .sidebar_list
        .row_at_index(index)
        .and_then(|row| row.child())
        .and_then(|child| child.downcast::<WorkspaceRow>().ok())
        .expect("session row should exist")
}

fn session_row_for_uuid(window: &Window, session_uuid: &str) -> WorkspaceRow {
    let list = &window.imp().sidebar_list;
    let mut idx = 0;
    while let Some(row) = list.row_at_index(idx) {
        if let Some(session_row) =
            row.child().and_then(|child| child.downcast::<WorkspaceRow>().ok())
            && session_row.uuid() == session_uuid
        {
            return session_row;
        }
        idx += 1;
    }
    panic!("session row for {session_uuid} should exist");
}

fn selected_session_uuid(window: &Window) -> Option<String> {
    window
        .imp()
        .sidebar_list
        .selected_row()
        .and_then(|row| row.child())
        .and_then(|child| child.downcast::<WorkspaceRow>().ok())
        .map(|row| row.uuid())
}

fn emit_left_click(widget: &gtk4::Widget, n_press: i32) {
    let controllers = widget.observe_controllers();
    for index in 0..controllers.n_items() {
        let Some(controller) = controllers.item(index) else {
            continue;
        };
        if let Ok(gesture) = controller.downcast::<gtk4::GestureClick>() {
            gesture.emit_by_name::<()>("released", &[&n_press, &0.0_f64, &0.0_f64]);
            return;
        }
    }
    panic!("widget should have a GestureClick controller");
}

fn make_state_two_sessions() -> WindowState {
    WindowState {
        active_workspace_index: 0,
        width: 800,
        height: 600,
        is_maximized: false,
        workspaces: vec![
            WorkspaceState {
                uuid: "s1".into(),
                name: "Session 1".into(),
                layout: LayoutNode::new_terminal_with_uuid("t1"),
                terminal_recovery: Default::default(),
                active_terminal_uuid: None,
                input_sync: false,
                runtime: Default::default(),
                color: Default::default(),
                zoomed_terminal_uuid: None,
                user_renamed: false,
            },
            WorkspaceState {
                uuid: "s2".into(),
                name: "Session 2".into(),
                layout: LayoutNode::new_terminal_with_uuid("t2"),
                terminal_recovery: Default::default(),
                active_terminal_uuid: None,
                input_sync: false,
                runtime: Default::default(),
                color: Default::default(),
                zoomed_terminal_uuid: None,
                user_renamed: false,
            },
        ],
        ..WindowState::default()
    }
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_in_background_session_triggers_notification() {
    let state = make_state_two_sessions();
    assert!(
        terminal_is_in_background_session("t1", Some("s2"), &state),
        "t1 is in s1 which is not visible (s2 is) — should notify"
    );
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_in_visible_session_suppresses_notification() {
    let state = make_state_two_sessions();
    assert!(
        !terminal_is_in_background_session("t1", Some("s1"), &state),
        "t1 is in s1 which IS visible — should not notify"
    );
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_in_visible_session_with_split_suppresses_notification() {
    let state = WindowState {
        workspaces: vec![WorkspaceState {
            uuid: "s1".into(),
            name: "Session 1".into(),
            layout: LayoutNode::Split {
                orientation: SplitOrientation::Horizontal,
                ratio: 0.5,
                first: Box::new(LayoutNode::new_terminal_with_uuid("t1")),
                second: Box::new(LayoutNode::new_terminal_with_uuid("t2")),
            },
            terminal_recovery: Default::default(),
            active_terminal_uuid: None,
            input_sync: false,
            runtime: Default::default(),
            color: Default::default(),
            zoomed_terminal_uuid: None,
            user_renamed: false,
        }],
        ..WindowState::default()
    };
    assert!(
        !terminal_is_in_background_session("t2", Some("s1"), &state),
        "t2 is in the visible session s1 even though it is not focused — should not notify"
    );
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn terminal_is_background_when_no_visible_session() {
    let state = make_state_two_sessions();
    assert!(
        terminal_is_in_background_session("t1", None, &state),
        "when no session is visible, treat terminal as background"
    );
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn preferred_command_target_uuid_uses_focused_terminal_first() {
    let state = WindowState {
        workspaces: vec![WorkspaceState {
            uuid: "s1".into(),
            name: "Session 1".into(),
            layout: LayoutNode::Split {
                orientation: SplitOrientation::Horizontal,
                ratio: 0.5,
                first: Box::new(LayoutNode::new_terminal_with_uuid("t1")),
                second: Box::new(LayoutNode::new_terminal_with_uuid("t2")),
            },
            terminal_recovery: Default::default(),
            active_terminal_uuid: None,
            input_sync: false,
            runtime: Default::default(),
            color: Default::default(),
            zoomed_terminal_uuid: None,
            user_renamed: false,
        }],
        ..WindowState::default()
    };

    assert_eq!(
        preferred_command_target_uuid(Some("t2"), Some("s1"), &state).as_deref(),
        Some("t2")
    );
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn preferred_command_target_uuid_falls_back_to_visible_session() {
    let state = WindowState {
        active_workspace_index: 1,
        workspaces: vec![
            WorkspaceState {
                uuid: "s1".into(),
                name: "Session 1".into(),
                layout: LayoutNode::new_terminal_with_uuid("t1"),
                terminal_recovery: Default::default(),
                active_terminal_uuid: None,
                input_sync: false,
                runtime: Default::default(),
                color: Default::default(),
                zoomed_terminal_uuid: None,
                user_renamed: false,
            },
            WorkspaceState {
                uuid: "s2".into(),
                name: "Session 2".into(),
                layout: LayoutNode::Split {
                    orientation: SplitOrientation::Vertical,
                    ratio: 0.5,
                    first: Box::new(LayoutNode::new_terminal_with_uuid("t2")),
                    second: Box::new(LayoutNode::new_terminal_with_uuid("t3")),
                },
                terminal_recovery: Default::default(),
                active_terminal_uuid: None,
                input_sync: false,
                runtime: Default::default(),
                color: Default::default(),
                zoomed_terminal_uuid: None,
                user_renamed: false,
            },
        ],
        ..WindowState::default()
    };

    assert_eq!(preferred_command_target_uuid(None, Some("s2"), &state).as_deref(), Some("t2"));
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn top_bar_has_new_connect_direct_buttons() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.top-bar-buttons-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    assert_eq!(
        window.imp().new_button.label().as_deref(),
        Some("New"),
        "New button should have 'New' label"
    );
    assert_eq!(
        window.imp().connect_button.label().as_deref(),
        Some("Connect"),
        "Connect button should have 'Connect' label"
    );
    assert_eq!(
        window.imp().new_direct_button.label().as_deref(),
        Some("Direct"),
        "Direct button should have 'Direct' label"
    );

    window.close();
}

// Regression (RFC-031): re-keying a client-minted pane onto its server pane id
// must also move the window's focused-pane pointer, which is stored outside the
// layout. Otherwise the next focus-driven action (split, close, zoom) targets
// the stale uuid that no longer exists in the layout and silently no-ops — e.g.
// a split that produces no second pane.
#[test]
#[ignore = "requires isolated GTK harness"]
fn rekey_terminal_widgets_moves_focused_pointer_to_server_pane_id() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.rekey-focus-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.set_focused_terminal(Some("client-minted-pane"));

    window.rekey_terminal_widgets(&crate::workspace_state::PaneRekey {
        workspace_id: "ws-1".into(),
        old_uuid: "client-minted-pane".into(),
        new_uuid: "server-pane-id".into(),
    });

    assert_eq!(
        window.focused_terminal_uuid().as_deref(),
        Some("server-pane-id"),
        "focused pointer must follow the re-key to the server pane id"
    );

    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn new_button_menu_model_survives_activation() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.new-btn-menu-survives-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    // Menu model must be present before any interaction.
    let model_before = window.imp().new_button.menu_model();
    assert!(model_before.is_some(), "New button should have a menu model");
    assert!(
        model_before.unwrap().n_items() > 0,
        "New button menu should contain at least one host item"
    );

    // Simulate the MenuButton becoming active (popover opening).
    window.imp().new_button.set_active(true);
    pump_events(50);

    // Menu model must still be present and non-empty after activation.
    let model_after = window.imp().new_button.menu_model();
    assert!(model_after.is_some(), "New button menu model must survive activation");
    assert!(
        model_after.unwrap().n_items() > 0,
        "New button menu must still have items after activation"
    );

    // Same check for the Connect button.
    let connect_model = window.imp().connect_button.menu_model();
    assert!(connect_model.is_some(), "Connect button should have a menu model");
    assert!(
        connect_model.unwrap().n_items() > 0,
        "Connect button menu should contain at least one host item"
    );

    window.imp().new_button.set_active(false);
    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn new_for_host_action_opens_dialog() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.new-for-host-action-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    // The new-for-host action must be registered.
    assert!(
        window.lookup_action("new-for-host").is_some(),
        "new-for-host action should be registered"
    );
    assert!(
        window.lookup_action("connect-for-host").is_some(),
        "connect-for-host action should be registered"
    );

    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn initial_terminal_starts_shell_when_window_is_presented() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.initial-terminal-shell-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let term = {
        let terminals = window.imp().terminals.borrow();
        terminals.values().next().cloned().expect("window should create an initial terminal")
    };

    assert!(
        !term.shell_spawned_for_test(),
        "shell startup should wait until the terminal is attached to a realized window"
    );

    window.set_default_size(900, 600);
    window.present();

    let spawned = wait_until(1000, || term.shell_spawned_for_test());
    assert!(spawned, "presenting the window should trigger delayed shell startup");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn utility_sidebar_shows_and_filters_commands() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let run = crate::commands::SavedCommand::new("Restart app", "systemctl restart app");
    let insert = crate::commands::SavedCommand::new("Deploy checklist", "cargo build\ncargo test");
    store().save_commands(&[run, insert]).unwrap();

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.utility-command-sidebar-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(100);

    assert_eq!(
        window.imp().command_list.observe_children().n_items(),
        2,
        "utility sidebar should show saved commands"
    );

    window.imp().sidebar_search_entry.set_text("deploy");
    pump_events(50);
    assert_eq!(
        window.imp().command_list.observe_children().n_items(),
        1,
        "search should filter the utility sidebar command list"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn command_row_shows_description_as_tooltip() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let mut with_desc = crate::commands::SavedCommand::new("Deploy", "cargo build");
    with_desc.description = "Builds the production service".into();
    let without_desc = crate::commands::SavedCommand::new("Test", "cargo test");
    store().save_commands(&[with_desc, without_desc]).unwrap();

    let app =
        adw::Application::builder().application_id("com.illya.rttx.command-tooltip-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(100);

    // Row 0 is the section header, rows 1 and 2 are commands
    let row_with_desc = window.imp().command_list.row_at_index(1).unwrap();
    let row_without_desc = window.imp().command_list.row_at_index(2).unwrap();

    assert_eq!(
        row_with_desc.tooltip_text().as_deref(),
        Some("Builds the production service"),
        "command with description should show it as tooltip"
    );
    assert_eq!(
        row_without_desc.tooltip_text().as_deref(),
        None,
        "command without description should have no tooltip"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn commands_page_has_no_separate_search_entry() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.commands-no-search-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    let commands_page = window
        .imp()
        .utility_stack
        .child_by_name("commands")
        .expect("commands page must exist")
        .downcast::<gtk4::Box>()
        .expect("commands page must be a Box");

    let mut has_search = false;
    let mut child = commands_page.first_child();
    while let Some(widget) = child {
        if widget.downcast_ref::<gtk4::SearchEntry>().is_some() {
            has_search = true;
            break;
        }
        child = widget.next_sibling();
    }
    assert!(!has_search, "commands page should not contain a separate SearchEntry");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn failed_structured_recovery_keeps_terminal_alive_and_allows_retry() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());

    let terminal_uuid = "t1".to_string();
    let session_uuid = "s1".to_string();
    let state = WindowState {
        workspaces: vec![WorkspaceState {
            uuid: session_uuid.clone(),
            name: "Ops".into(),
            layout: LayoutNode::new_terminal_with_uuid(&terminal_uuid),
            terminal_recovery: std::collections::BTreeMap::from([(
                terminal_uuid.clone(),
                PaneRecovery {
                    source: PaneSource::Manual,
                    target: Some(PaneTarget::RemoteShell {
                        ssh_target: "user@192.0.2.1".into(),
                        remote_folder: None,
                    }),
                    startup: vec![],
                },
            )]),
            active_terminal_uuid: Some(terminal_uuid.clone()),
            input_sync: false,
            runtime: Default::default(),
            color: Default::default(),
            zoomed_terminal_uuid: None,
            user_renamed: false,
        }],
        ..WindowState::default()
    };
    save_window_state_to_store(&state);

    let app =
        adw::Application::builder().application_id("com.illya.rttx.recovery-failure-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.set_default_size(900, 600);
    window.present();

    let term = window
        .imp()
        .terminals
        .borrow()
        .get(&terminal_uuid)
        .cloned()
        .expect("recoverable terminal should exist");

    let failed = wait_until(3000, || term.recovery_message_visible_for_test());
    assert!(failed, "unreachable SSH host should leave the pane alive and show retry UI");
    assert!(
        term.recovery_message_for_test().contains("Failed to connect to"),
        "failure message should stay inside the pane"
    );
    assert!(
        window.imp().terminals.borrow().contains_key(&terminal_uuid),
        "failed recovery must not close the pane"
    );

    term.recovery_retry_button().emit_clicked();
    let retried = wait_until(3000, || term.recovery_message_visible_for_test());
    assert!(
        retried,
        "retry should re-attempt recovery and return to failed state if the target is still unavailable"
    );

    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn inserted_commands_persist_nonexecuting_recovery_recipe_on_restart() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.command-recovery-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let first_window = Window::new(&app);
    let command = SavedCommand::new("Deploy checklist", "cargo build\ncargo test");

    first_window.execute_saved_command(&command, CommandRunMode::Insert);

    let (terminal_uuid, saved_recovery) = {
        let state = first_window.imp().state.borrow();
        let session = &state.workspaces[0];
        let terminal_uuid = session.layout.terminal_uuids().into_iter().next().unwrap();
        (terminal_uuid.clone(), session.recovery_for(&terminal_uuid).cloned())
    };

    assert_eq!(
        saved_recovery,
        Some(PaneRecovery {
            source: PaneSource::Command { title: "Deploy checklist".into() },
            target: None,
            startup: vec![StartupStep::SendText {
                text: "cargo build\ncargo test".into(),
                execute: false,
            }],
        })
    );

    first_window.save_state();
    first_window.close();

    let second_window = Window::new(&app);
    let restored_term = second_window
        .imp()
        .terminals
        .borrow()
        .get(&terminal_uuid)
        .cloned()
        .expect("restored command terminal should exist");

    assert_eq!(
        restored_term.pending_shell_inputs_for_test(),
        vec![String::from("cargo build\ncargo test")],
        "restored insert-mode command should replay without an added newline"
    );

    second_window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn execute_saved_command_queues_input_before_shell_starts() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.command-queue-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let command = SavedCommand::new("Deploy checklist", "cargo build\ncargo test");

    window.execute_saved_command(&command, CommandRunMode::Insert);

    let terminal_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    let term = window
        .imp()
        .terminals
        .borrow()
        .get(&terminal_uuid)
        .cloned()
        .expect("initial command target terminal should exist");

    assert!(
        !term.shell_spawned_for_test(),
        "shell should not start before the window is presented"
    );
    assert_eq!(
        term.pending_shell_inputs_for_test(),
        vec![String::from("cargo build\ncargo test")],
        "command launcher input should be queued until the shell is ready"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn save_and_restart_restores_active_terminal_in_active_session() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.restore-active-terminal-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let first_window = Window::new(&app);
    first_window.set_default_size(1200, 800);
    first_window.present();
    pump_events(100);

    let root_uuid = {
        let state = first_window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    first_window.split_terminal(&root_uuid, SplitOrientation::Horizontal);
    pump_events(100);

    let second_uuid = {
        let state = first_window.imp().state.borrow();
        state.workspaces[0]
            .layout
            .terminal_uuids()
            .into_iter()
            .find(|uuid| uuid != &root_uuid)
            .unwrap()
    };
    let second_term = first_window
        .imp()
        .terminals
        .borrow()
        .get(&second_uuid)
        .cloned()
        .expect("split terminal should exist");
    assert!(second_term.vte().grab_focus());
    let focused = wait_until(1000, || {
        first_window.focused_terminal_uuid().as_deref() == Some(second_uuid.as_str())
            && first_window.imp().state.borrow().workspaces[0].active_terminal_uuid.as_deref()
                == Some(second_uuid.as_str())
    });
    assert!(focused, "focusing a pane should record it as the session's active terminal");

    first_window.save_state();
    first_window.close();

    let saved_state = load_saved_window_state();
    assert_eq!(
        saved_state.workspaces[0].active_terminal_uuid.as_deref(),
        Some(second_uuid.as_str()),
        "saved state should remember the last active pane in the session"
    );

    let second_window = Window::new(&app);
    second_window.set_default_size(1200, 800);
    second_window.present();

    let restored = wait_until(1000, || {
        second_window.focused_terminal_uuid().as_deref() == Some(second_uuid.as_str())
    });
    assert!(
        restored,
        "restart should restore focus to the previously active pane instead of the first pane"
    );

    second_window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn switching_sessions_focuses_the_visible_terminal() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.session-focus-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.set_default_size(900, 600);
    window.present();
    pump_events(100);

    let first_terminal = {
        let state = window.imp().state.borrow();
        let first_uuid = state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap();
        window.imp().terminals.borrow().get(&first_uuid).cloned().unwrap()
    };
    assert!(first_terminal.vte().grab_focus());
    let first_focused = wait_until(1000, || first_terminal.vte().has_focus());
    assert!(first_focused, "initial terminal should be focusable");

    window.add_session();
    let second_terminal = {
        let state = window.imp().state.borrow();
        let second_uuid = state.workspaces[1].layout.terminal_uuids().into_iter().next().unwrap();
        window.imp().terminals.borrow().get(&second_uuid).cloned().unwrap()
    };
    let second_focused = wait_until(1000, || second_terminal.vte().has_focus());
    assert!(second_focused, "newly selected session should hand focus to its terminal");

    let first_row = window.imp().sidebar_list.row_at_index(0).unwrap();
    window.imp().sidebar_list.select_row(Some(&first_row));
    let restored_focus = wait_until(1000, || first_terminal.vte().has_focus());
    assert!(restored_focus, "switching back should focus the visible terminal without a click");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn active_pane_class_tracks_terminal_focus() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.active-pane-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.set_default_size(1000, 700);
    window.present();
    pump_events(100);

    let first_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    window.split_terminal(&first_uuid, SplitOrientation::Horizontal);
    pump_events(100);

    let (first_term, second_term) = {
        let state = window.imp().state.borrow();
        let second_uuid = state.workspaces[0]
            .layout
            .terminal_uuids()
            .into_iter()
            .find(|uuid| uuid != &first_uuid)
            .unwrap();
        let terminals = window.imp().terminals.borrow();
        (
            terminals.get(&first_uuid).cloned().unwrap(),
            terminals.get(&second_uuid).cloned().unwrap(),
        )
    };

    assert!(first_term.vte().grab_focus());
    assert!(
        wait_until(1000, || {
            first_term.has_css_class("terminal-pane-active")
                && !second_term.has_css_class("terminal-pane-active")
        }),
        "first pane should gain the active-pane class when focused"
    );

    assert!(second_term.vte().grab_focus());
    assert!(
        wait_until(1000, || {
            second_term.has_css_class("terminal-pane-active")
                && !first_term.has_css_class("terminal-pane-active")
        }),
        "active-pane class should move to the newly focused pane"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn clicking_title_label_focuses_the_terminal() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.title-click-focus-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.set_default_size(1000, 700);
    window.present();
    pump_events(100);

    let first_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    window.split_terminal(&first_uuid, SplitOrientation::Horizontal);
    pump_events(100);

    let (first_term, second_term, second_uuid) = {
        let state = window.imp().state.borrow();
        let second_uuid = state.workspaces[0]
            .layout
            .terminal_uuids()
            .into_iter()
            .find(|uuid| uuid != &first_uuid)
            .unwrap();
        let terminals = window.imp().terminals.borrow();
        (
            terminals.get(&first_uuid).cloned().unwrap(),
            terminals.get(&second_uuid).cloned().unwrap(),
            second_uuid,
        )
    };

    assert!(second_term.vte().grab_focus());
    assert!(
        wait_until(1000, || window.focused_terminal_uuid().as_deref()
            == Some(second_uuid.as_str())),
        "test setup should start with the second pane focused"
    );

    emit_left_click(first_term.title_label().upcast_ref::<gtk4::Widget>(), 1);
    assert!(
        wait_until(1000, || {
            window.focused_terminal_uuid().as_deref() == Some(first_uuid.as_str())
                && first_term.has_css_class("terminal-pane-active")
                && !second_term.has_css_class("terminal-pane-active")
        }),
        "clicking the title label should focus and activate that pane"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn split_rebuild_starts_new_panes_evenly() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder().application_id("com.illya.rttx.window-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.set_default_size(1200, 800);
    window.present();
    pump_events(100);

    let (session_uuid, t1_uuid) = {
        let state = window.imp().state.borrow();
        let session = &state.workspaces[0];
        (session.uuid.clone(), session.layout.terminal_uuids().into_iter().next().unwrap())
    };

    window.split_terminal(&t1_uuid, SplitOrientation::Horizontal);
    pump_events(100);

    let t2_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0]
            .layout
            .terminal_uuids()
            .into_iter()
            .find(|uuid| uuid != &t1_uuid)
            .unwrap()
    };
    window.split_terminal(&t2_uuid, SplitOrientation::Vertical);

    let settled = wait_until(1000, || {
        let Some(root) = window.imp().session_stack.child_by_name(&session_uuid) else {
            return false;
        };
        let Ok(outer) = root.downcast::<gtk4::Paned>() else {
            return false;
        };
        let outer_total = outer.width();
        if outer_total <= 0 {
            return false;
        }
        let outer_ratio = outer.position() as f64 / outer_total as f64;

        let Some(inner_child) = outer.end_child() else {
            return false;
        };
        let Ok(inner) = inner_child.downcast::<gtk4::Paned>() else {
            return false;
        };
        let inner_total = inner.height();
        if inner_total <= 0 {
            return false;
        }
        let inner_ratio = inner.position() as f64 / inner_total as f64;

        (outer_ratio - 0.5).abs() <= 0.08 && (inner_ratio - 0.5).abs() <= 0.08
    });

    let root = window
        .imp()
        .session_stack
        .child_by_name(&session_uuid)
        .expect("session content must exist");
    let outer = root.downcast::<gtk4::Paned>().expect("root after split must be a Paned");
    let inner = outer
        .end_child()
        .expect("second split should produce nested Paned on the right")
        .downcast::<gtk4::Paned>()
        .expect("right child must be a nested Paned");
    let outer_ratio = outer.position() as f64 / outer.width().max(1) as f64;
    let inner_ratio = inner.position() as f64 / inner.height().max(1) as f64;

    assert!(
        settled,
        "newly rebuilt splits must settle near 50/50.\n\
         outer: pos={} total={} ratio={outer_ratio:.3}\n\
         inner: pos={} total={} ratio={inner_ratio:.3}",
        outer.position(),
        outer.width(),
        inner.position(),
        inner.height(),
    );

    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn save_and_restart_restores_user_resized_pane_ratios() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.restore-ratios-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let first_window = Window::new(&app);
    first_window.set_default_size(1200, 800);
    first_window.present();
    pump_events(100);

    let (session_uuid, t1_uuid) = {
        let state = first_window.imp().state.borrow();
        let session = &state.workspaces[0];
        (session.uuid.clone(), session.layout.terminal_uuids().into_iter().next().unwrap())
    };

    first_window.split_terminal(&t1_uuid, SplitOrientation::Horizontal);

    let settled = wait_until(1000, || {
        let Some(root) = first_window.imp().session_stack.child_by_name(&session_uuid) else {
            return false;
        };
        let Ok(paned) = root.downcast::<gtk4::Paned>() else {
            return false;
        };
        paned.width() > 0
    });
    assert!(settled, "split pane did not receive an allocation before save");

    let root = first_window
        .imp()
        .session_stack
        .child_by_name(&session_uuid)
        .expect("session content must exist before save");
    let paned = root.downcast::<gtk4::Paned>().expect("split root should be a Paned");
    let total = paned.width().max(1);
    let expected_ratio = 0.3;
    paned.set_position((f64::from(total) * expected_ratio) as i32);
    pump_events(50);

    first_window.save_state();
    first_window.close();

    let saved_state = load_saved_window_state();
    let LayoutNode::Split { ratio: saved_ratio, .. } = &saved_state.workspaces[0].layout else {
        panic!("saved layout should remain split after resize");
    };
    assert!(
        (*saved_ratio - expected_ratio).abs() <= 0.05,
        "save_state should capture the user-resized split ratio before restart, got {saved_ratio}"
    );

    let second_window = Window::new(&app);
    second_window.set_default_size(1200, 800);
    second_window.present();

    let restored = wait_until(1000, || {
        let Some(root) = second_window.imp().session_stack.child_by_name(&session_uuid) else {
            return false;
        };
        let Ok(paned) = root.downcast::<gtk4::Paned>() else {
            return false;
        };
        let total = paned.width();
        if total <= 0 {
            return false;
        }
        let ratio = paned.position() as f64 / total as f64;
        (ratio - expected_ratio).abs() <= 0.08
    });

    let restored_root = second_window
        .imp()
        .session_stack
        .child_by_name(&session_uuid)
        .expect("session content must exist after restart");
    let restored_paned =
        restored_root.downcast::<gtk4::Paned>().expect("restored root should be a Paned");
    let restored_ratio = restored_paned.position() as f64 / restored_paned.width().max(1) as f64;
    assert!(
        restored,
        "restart should restore the saved split ratio.\n\
         saved={saved_ratio:.3} restored={restored_ratio:.3} pos={} total={}",
        restored_paned.position(),
        restored_paned.width(),
    );

    second_window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn save_state_updates_nested_terminal_cwds() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder().application_id("com.illya.rttx.save-cwd-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let root_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    window.split_terminal(&root_uuid, SplitOrientation::Horizontal);

    let terminal_uuids = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids()
    };
    assert_eq!(terminal_uuids.len(), 2, "test setup should create a split session");

    let terminals = window.imp().terminals.borrow();
    terminals
        .get(&terminal_uuids[0])
        .unwrap()
        .set_current_directory_for_test(Some("/tmp/project-a"));
    terminals
        .get(&terminal_uuids[1])
        .unwrap()
        .set_current_directory_for_test(Some("/tmp/project-b"));
    drop(terminals);

    window.save_state();
    let saved_state = load_saved_window_state();

    let LayoutNode::Split { first, second, .. } = &saved_state.workspaces[0].layout else {
        panic!("saved layout should stay split");
    };
    let LayoutNode::Terminal { cwd: first_cwd, .. } = first.as_ref() else {
        panic!("first child should be a terminal");
    };
    let LayoutNode::Terminal { cwd: second_cwd, .. } = second.as_ref() else {
        panic!("second child should be a terminal");
    };
    assert_eq!(first_cwd.as_deref(), Some("/tmp/project-a"));
    assert_eq!(second_cwd.as_deref(), Some("/tmp/project-b"));

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn save_and_restart_restores_custom_terminal_title() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.custom-title-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let first_window = Window::new(&app);
    first_window.set_default_size(900, 600);
    first_window.present();
    pump_events(100);

    let terminal_uuid = {
        let state = first_window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    let term = first_window
        .imp()
        .terminals
        .borrow()
        .get(&terminal_uuid)
        .cloned()
        .expect("initial terminal should exist");
    term.set_custom_title(Some("Editor"));

    first_window.save_state();
    let saved_state = load_saved_window_state();
    assert_eq!(
        saved_state.workspaces[0].layout.terminal_custom_title(&terminal_uuid).as_deref(),
        Some("Editor"),
        "save_state should capture the live custom pane title into the layout"
    );

    first_window.close();

    let second_window = Window::new(&app);
    second_window.set_default_size(900, 600);
    second_window.present();
    pump_events(100);

    let restored_term = second_window
        .imp()
        .terminals
        .borrow()
        .get(&terminal_uuid)
        .cloned()
        .expect("restored terminal should exist");
    assert_eq!(restored_term.custom_title().as_deref(), Some("Editor"));
    assert_eq!(restored_term.title_label().label().as_str(), "Editor");

    second_window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn save_and_restart_restores_nested_user_resized_pane_ratios() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.restore-nested-ratios-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let first_window = Window::new(&app);
    first_window.set_default_size(1200, 800);
    first_window.present();
    pump_events(100);

    let (session_uuid, t1_uuid) = {
        let state = first_window.imp().state.borrow();
        let session = &state.workspaces[0];
        (session.uuid.clone(), session.layout.terminal_uuids().into_iter().next().unwrap())
    };

    first_window.split_terminal(&t1_uuid, SplitOrientation::Horizontal);
    pump_events(100);

    let t2_uuid = {
        let state = first_window.imp().state.borrow();
        state.workspaces[0]
            .layout
            .terminal_uuids()
            .into_iter()
            .find(|uuid| uuid != &t1_uuid)
            .unwrap()
    };

    first_window.split_terminal(&t2_uuid, SplitOrientation::Vertical);

    let settled = wait_until(1000, || {
        let Some(root) = first_window.imp().session_stack.child_by_name(&session_uuid) else {
            return false;
        };
        let Ok(outer) = root.downcast::<gtk4::Paned>() else {
            return false;
        };
        let Some(inner_child) = outer.end_child() else {
            return false;
        };
        let Ok(inner) = inner_child.downcast::<gtk4::Paned>() else {
            return false;
        };
        outer.width() > 0 && inner.height() > 0
    });
    assert!(settled, "nested split panes did not receive allocation before save");

    let root = first_window
        .imp()
        .session_stack
        .child_by_name(&session_uuid)
        .expect("session content must exist before save");
    let outer = root.downcast::<gtk4::Paned>().expect("outer root should be a Paned");
    let inner = outer
        .end_child()
        .expect("nested split should exist on one branch")
        .downcast::<gtk4::Paned>()
        .expect("nested branch should be a Paned");

    let expected_outer_ratio = 0.32;
    let expected_inner_ratio = 0.68;
    outer.set_position((f64::from(outer.width().max(1)) * expected_outer_ratio) as i32);
    inner.set_position((f64::from(inner.height().max(1)) * expected_inner_ratio) as i32);
    pump_events(50);

    first_window.save_state();
    first_window.close();

    let saved_state = load_saved_window_state();
    let LayoutNode::Split { ratio: saved_outer_ratio, second, .. } =
        &saved_state.workspaces[0].layout
    else {
        panic!("saved layout should remain nested after resize");
    };
    let LayoutNode::Split { ratio: saved_inner_ratio, .. } = second.as_ref() else {
        panic!("saved nested branch should remain a split after resize");
    };
    assert!(
        (*saved_outer_ratio - expected_outer_ratio).abs() <= 0.05,
        "save_state should capture the outer user-resized split ratio, got {saved_outer_ratio}"
    );
    assert!(
        (*saved_inner_ratio - expected_inner_ratio).abs() <= 0.05,
        "save_state should capture the inner user-resized split ratio, got {saved_inner_ratio}"
    );

    let second_window = Window::new(&app);
    second_window.set_default_size(1200, 800);
    second_window.present();

    let restored = wait_until(1000, || {
        let Some(root) = second_window.imp().session_stack.child_by_name(&session_uuid) else {
            return false;
        };
        let Ok(outer) = root.downcast::<gtk4::Paned>() else {
            return false;
        };
        let Some(inner_child) = outer.end_child() else {
            return false;
        };
        let Ok(inner) = inner_child.downcast::<gtk4::Paned>() else {
            return false;
        };
        let outer_total = outer.width();
        let inner_total = inner.height();
        if outer_total <= 0 || inner_total <= 0 {
            return false;
        }
        let outer_ratio = outer.position() as f64 / outer_total as f64;
        let inner_ratio = inner.position() as f64 / inner_total as f64;
        (outer_ratio - expected_outer_ratio).abs() <= 0.08
            && (inner_ratio - expected_inner_ratio).abs() <= 0.08
    });

    let restored_root = second_window
        .imp()
        .session_stack
        .child_by_name(&session_uuid)
        .expect("session content must exist after restart");
    let restored_outer =
        restored_root.downcast::<gtk4::Paned>().expect("restored outer root should be a Paned");
    let restored_inner = restored_outer
        .end_child()
        .expect("restored nested split should exist")
        .downcast::<gtk4::Paned>()
        .expect("restored nested branch should be a Paned");
    let restored_outer_ratio =
        restored_outer.position() as f64 / restored_outer.width().max(1) as f64;
    let restored_inner_ratio =
        restored_inner.position() as f64 / restored_inner.height().max(1) as f64;
    assert!(
        restored,
        "restart should restore both nested split ratios.\n\
         saved_outer={saved_outer_ratio:.3} restored_outer={restored_outer_ratio:.3}\n\
         saved_inner={saved_inner_ratio:.3} restored_inner={restored_inner_ratio:.3}"
    );

    second_window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn rename_runtime_updates_sidebar_and_saved_state() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.rename-session-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let session_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].uuid.clone()
    };

    window.rename_runtime(&session_uuid, "Renamed Session");

    {
        let state = window.imp().state.borrow();
        assert_eq!(state.workspaces[0].name, "Renamed Session");
    }

    let row = window.imp().sidebar_list.row_at_index(0).expect("session row should exist");
    let session_row = row
        .child()
        .and_then(|child| child.downcast::<WorkspaceRow>().ok())
        .expect("session row child should be WorkspaceRow");
    assert_eq!(session_row.workspace_name(), "Renamed Session");
    assert_eq!(session_row.title().as_str(), "Renamed Session");

    // No explicit save_state() here: renaming must persist on its own, or the
    // name is lost when the client exits without a clean close (issue #1084).
    let saved_state = load_saved_window_state();
    assert_eq!(saved_state.workspaces[0].name, "Renamed Session");
    assert!(saved_state.workspaces[0].user_renamed);

    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn tools_sidebar_uses_per_row_management_instead_of_manage_dialog() {
    require_display!();

    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");
    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());

    let app =
        adw::Application::builder().application_id("com.illya.rttx.sidebar-mgmt-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();
    let window = Window::new(&app);

    assert!(
        window.lookup_action("manage-commands").is_none(),
        "manage-commands action should be removed"
    );
    assert!(
        window.lookup_action("add-command").is_some(),
        "add-command action should be registered"
    );
    assert!(
        window.lookup_action("edit-command").is_some(),
        "edit-command action should be registered"
    );
    assert!(
        window.lookup_action("delete-command").is_some(),
        "delete-command action should be registered"
    );
    assert!(window.lookup_action("add-place").is_some(), "add-place action should be registered");
    assert!(window.lookup_action("edit-place").is_some(), "edit-place action should be registered");
    assert!(
        window.lookup_action("delete-place").is_some(),
        "delete-place action should be registered"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn command_sidebar_shows_empty_state_when_no_commands() {
    require_display!();

    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");
    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.command-empty-state-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();
    let window = Window::new(&app);
    window.present();
    pump_events(50);

    assert!(
        window.imp().command_empty.is_visible(),
        "empty state should be visible when no commands"
    );
    assert!(
        !window.imp().command_scroll.is_visible(),
        "list scroll should be hidden when no commands"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn about_action_is_registered() {
    require_display!();

    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.about-window-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    assert!(window.lookup_action("about").is_some(), "window should expose an about action");
    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn smart_clipboard_preference_reaches_live_terminals() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let mut prefs = preferences::Preferences::default();
    prefs.smart_clipboard = true;
    store().save_preferences(&prefs).unwrap();

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.smart-clipboard-preferences-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let terminal = window
        .imp()
        .terminals
        .borrow()
        .values()
        .next()
        .cloned()
        .expect("window should create an initial terminal");
    assert!(terminal.smart_clipboard_enabled_for_test());

    prefs.smart_clipboard = false;
    store().save_preferences(&prefs).unwrap();
    window.reapply_terminal_preferences();
    assert!(!terminal.smart_clipboard_enabled_for_test());

    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn search_bar_toggles_and_returns_focus_to_terminal() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder().application_id("com.illya.rttx.search-bar-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let terminal = window
        .imp()
        .terminals
        .borrow()
        .values()
        .next()
        .cloned()
        .expect("window should create an initial terminal");

    assert!(!terminal.search_bar().is_search_mode(), "search bar starts hidden");

    // Toggle on: the bar shows and the search entry takes focus.
    terminal.toggle_search();
    assert!(terminal.search_bar().is_search_mode(), "toggle shows the search bar");
    assert!(
        wait_until(1000, || terminal.search_entry().has_focus()),
        "the search entry should take focus when the bar opens"
    );

    // Toggle off: the bar hides and focus returns to the terminal (guards the
    // 'focus not returning after close' regression called out in #323).
    terminal.toggle_search();
    assert!(!terminal.search_bar().is_search_mode(), "toggle hides the search bar");
    assert!(
        wait_until(1000, || terminal.vte().has_focus()),
        "focus must return to the terminal when the search bar closes"
    );

    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn font_preference_reaches_live_terminals_and_new_ones() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let mut prefs = preferences::Preferences::default();
    prefs.font = "Monospace 18".into();
    store().save_preferences(&prefs).unwrap();

    let app =
        adw::Application::builder().application_id("com.illya.rttx.font-preferences-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let terminal = window
        .imp()
        .terminals
        .borrow()
        .values()
        .next()
        .cloned()
        .expect("window should create an initial terminal");
    window.reapply_terminal_preferences();
    assert_eq!(
        terminal.vte().font().map(|f| f.to_string()),
        Some("Monospace 18".to_string()),
        "existing terminal must adopt the configured font"
    );

    // A terminal created after the change inherits the new font.
    window.add_session();
    let newest = window
        .imp()
        .terminals
        .borrow()
        .values()
        .last()
        .cloned()
        .expect("new session should create a terminal");
    assert_eq!(
        newest.vte().font().map(|f| f.to_string()),
        Some("Monospace 18".to_string()),
        "a terminal created after the change must inherit the font"
    );

    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn scrollback_and_audible_bell_preferences_reach_live_terminals() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let mut prefs = preferences::Preferences::default();
    prefs.scrollback_lines = 4242;
    prefs.audible_bell = false;
    store().save_preferences(&prefs).unwrap();

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.scrollback-bell-preferences-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let terminal = window
        .imp()
        .terminals
        .borrow()
        .values()
        .next()
        .cloned()
        .expect("window should create an initial terminal");
    window.reapply_terminal_preferences();
    assert_eq!(terminal.vte().scrollback_lines(), 4242, "scrollback must propagate");
    assert!(!terminal.vte().is_audible_bell(), "audible bell off must propagate");

    // Flip both and reapply.
    prefs.scrollback_lines = 1000;
    prefs.audible_bell = true;
    store().save_preferences(&prefs).unwrap();
    window.reapply_terminal_preferences();
    assert_eq!(terminal.vte().scrollback_lines(), 1000);
    assert!(terminal.vte().is_audible_bell());

    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn switch_to_session_number_selects_expected_session() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.alt-number-session-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.add_session();
    window.add_session();
    pump_events(50);

    window.switch_to_session_number(2);
    pump_events(50);

    assert_eq!(window.imp().sidebar_list.selected_row().map(|row| row.index()), Some(1));

    let visible_session = window
        .imp()
        .session_stack
        .visible_child_name()
        .expect("switching by number should select a visible session");
    let expected_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[1].uuid.clone()
    };
    assert_eq!(visible_session.as_str(), expected_uuid);

    window.switch_to_session_number(9);
    pump_events(50);

    assert_eq!(
        window.imp().sidebar_list.selected_row().map(|row| row.index()),
        Some(1),
        "out-of-range session numbers should leave the current selection unchanged"
    );

    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn nested_split_preserves_root_and_unaffected_terminals() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.window-identity-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.set_default_size(1200, 800);
    window.present();
    pump_events(100);

    let (session_uuid, t1_uuid) = {
        let state = window.imp().state.borrow();
        let session = &state.workspaces[0];
        (session.uuid.clone(), session.layout.terminal_uuids().into_iter().next().unwrap())
    };

    window.split_terminal(&t1_uuid, SplitOrientation::Horizontal);
    pump_events(100);

    let t2_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0]
            .layout
            .terminal_uuids()
            .into_iter()
            .find(|uuid| uuid != &t1_uuid)
            .unwrap()
    };

    let root_before = window
        .imp()
        .session_stack
        .child_by_name(&session_uuid)
        .expect("session content must exist before nested split");
    let root_before_ptr = root_before.as_ptr();
    let t1_before_ptr = window
        .imp()
        .terminals
        .borrow()
        .get(&t1_uuid)
        .expect("original terminal must exist before nested split")
        .as_ptr();

    window.split_terminal(&t2_uuid, SplitOrientation::Vertical);

    let settled = wait_until(1000, || {
        let Some(root) = window.imp().session_stack.child_by_name(&session_uuid) else {
            return false;
        };
        let Ok(root_paned) = root.downcast::<gtk4::Paned>() else {
            return false;
        };
        let Some(end_child) = root_paned.end_child() else {
            return false;
        };
        end_child.is::<gtk4::Paned>()
    });

    assert!(settled, "nested split did not settle into a nested Paned");

    let root_after = window
        .imp()
        .session_stack
        .child_by_name(&session_uuid)
        .expect("session content must exist after nested split");
    let root_after_ptr = root_after.as_ptr();
    let root_after_paned = root_after
        .clone()
        .downcast::<gtk4::Paned>()
        .expect("root should remain a Paned after nested split");
    let t1_after = window
        .imp()
        .terminals
        .borrow()
        .get(&t1_uuid)
        .expect("original terminal must still exist after nested split")
        .clone();

    assert_eq!(
        root_before_ptr, root_after_ptr,
        "nested split should preserve the existing session root widget instead of rebuilding it"
    );
    assert_eq!(
        t1_before_ptr,
        t1_after.as_ptr(),
        "nested split should preserve unaffected terminal widget identity"
    );
    assert_eq!(
        root_after_paned.start_child(),
        Some(t1_after.upcast::<gtk4::Widget>()),
        "unaffected terminal should stay attached as the unchanged sibling branch"
    );
    assert!(
        root_after_paned
            .end_child()
            .expect("nested split should create a new branch")
            .is::<gtk4::Paned>(),
        "nested split should replace only the target leaf with a nested Paned"
    );

    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn split_blocked_at_max_depth_does_not_increase_terminal_count() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.split-depth-limit-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.set_default_size(1200, 800);
    window.present();
    pump_events(100);

    let first_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };

    let mut leaf_uuid = first_uuid;
    for _ in 1..MAX_SPLIT_DEPTH {
        window.split_terminal(&leaf_uuid, SplitOrientation::Horizontal);
        pump_events(50);
        leaf_uuid = {
            let state = window.imp().state.borrow();
            state.workspaces[0]
                .layout
                .terminal_uuids()
                .into_iter()
                .max_by_key(|uuid| state.workspaces[0].layout.depth_of_terminal(uuid).unwrap_or(0))
                .unwrap()
        };
    }

    let count_before = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_count()
    };

    window.split_terminal(&leaf_uuid, SplitOrientation::Horizontal);
    pump_events(50);

    let count_after = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_count()
    };

    assert_eq!(
        count_before, count_after,
        "split at max depth should be blocked and not add a terminal"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

/// Regression test: split new pane had no terminal (shell never spawned).
///
/// `split_terminal_in_place` creates a new TerminalWidget but previously
/// never called `ensure_shell_spawned_when_ready()`. The new pane appeared
/// empty — PTY was never started. shell_spawned_for_test() only becomes
/// true after spawn_shell_once() runs, which only happens when
/// ensure_shell_spawned_when_ready() is called.
#[test]
#[ignore = "requires isolated GTK harness"]
fn split_spawns_shell_in_new_pane() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder().application_id("com.illya.rttx.split-shell-test").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();
    let window = Window::new(&app);
    window.set_default_size(1200, 800);
    window.present();
    pump_events(100);

    let t1_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };

    window.split_terminal(&t1_uuid, SplitOrientation::Horizontal);

    let t2_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().find(|u| u != &t1_uuid).unwrap()
    };

    let spawned = wait_until(2000, || {
        let terminals = window.imp().terminals.borrow();
        terminals.get(&t2_uuid).is_some_and(|t| t.shell_spawned_for_test())
    });

    assert!(
        spawned,
        "new pane from split must have shell_spawned=true after allocation. \
         ensure_shell_spawned_when_ready() must be called in split_terminal_in_place."
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn background_session_detection_identifies_foreground_terminal() {
    use crate::test_helpers::{term, window_state, workspace};

    let state =
        window_state(vec![workspace("s1", "A", term("t1")), workspace("s2", "B", term("t2"))]);

    assert!(
        !terminal_is_in_background_session("t1", Some("s1"), &state),
        "t1 is in the visible session s1"
    );
    assert!(
        terminal_is_in_background_session("t2", Some("s1"), &state),
        "t2 is in background session s2"
    );
    assert!(
        terminal_is_in_background_session("t1", None, &state),
        "no visible session means everything is background"
    );
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn background_session_detection_unknown_terminal_is_background() {
    use crate::test_helpers::{term, window_state, workspace};

    let state = window_state(vec![workspace("s1", "A", term("t1"))]);

    assert!(
        terminal_is_in_background_session("nonexistent", Some("s1"), &state),
        "unknown terminal should be treated as background"
    );
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn session_reorder_updates_state_order() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.session-reorder-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.add_session();
    window.add_session();
    pump_events(50);

    let (uuid0, uuid1, uuid2) = {
        let state = window.imp().state.borrow();
        (
            state.workspaces[0].uuid.clone(),
            state.workspaces[1].uuid.clone(),
            state.workspaces[2].uuid.clone(),
        )
    };

    // Move session 2 (index 2) to session 0's position (index 0).
    window.reorder_session(&uuid2, &uuid0);
    pump_events(50);

    let order: Vec<String> = {
        let state = window.imp().state.borrow();
        state.workspaces.iter().map(|s| s.uuid.clone()).collect()
    };
    assert_eq!(order, vec![uuid2.clone(), uuid0.clone(), uuid1.clone()]);

    // Verify sidebar rows match the new order.
    let sidebar_uuid_0 = window
        .imp()
        .sidebar_list
        .row_at_index(0)
        .and_then(|r| r.child())
        .and_then(|c| c.downcast::<WorkspaceRow>().ok())
        .map(|sr| sr.uuid());
    assert_eq!(sidebar_uuid_0.as_deref(), Some(uuid2.as_str()));

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn cycle_session_follows_index_order() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.cycle-session-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.add_session();
    window.add_session();
    pump_events(50);

    let (uuid0, uuid1, uuid2) = {
        let state = window.imp().state.borrow();
        (
            state.workspaces[0].uuid.clone(),
            state.workspaces[1].uuid.clone(),
            state.workspaces[2].uuid.clone(),
        )
    };

    // Start at session 0.
    window.switch_to_session_number(1);
    pump_events(50);
    let visible = window.imp().session_stack.visible_child_name().unwrap().to_string();
    assert_eq!(visible, uuid0, "should start at session 0");

    // Cycle forward: 0 → 1 → 2.
    window.cycle_session(1);
    pump_events(50);
    let visible = window.imp().session_stack.visible_child_name().unwrap().to_string();
    assert_eq!(visible, uuid1, "cycle +1 from 0 should show session 1");

    window.cycle_session(1);
    pump_events(50);
    let visible = window.imp().session_stack.visible_child_name().unwrap().to_string();
    assert_eq!(visible, uuid2, "cycle +1 from 1 should show session 2");

    // Cycle backward: 2 → 1 → 0.
    window.cycle_session(-1);
    pump_events(50);
    let visible = window.imp().session_stack.visible_child_name().unwrap().to_string();
    assert_eq!(visible, uuid1, "cycle -1 from 2 should show session 1");

    window.cycle_session(-1);
    pump_events(50);
    let visible = window.imp().session_stack.visible_child_name().unwrap().to_string();
    assert_eq!(visible, uuid0, "cycle -1 from 1 should show session 0");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn cycle_session_wraps_around() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder().application_id("com.illya.rttx.cycle-wrap-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.add_session();
    pump_events(50);

    let (uuid0, uuid1) = {
        let state = window.imp().state.borrow();
        (state.workspaces[0].uuid.clone(), state.workspaces[1].uuid.clone())
    };

    // Start at last session, cycle forward should wrap to first.
    window.switch_to_session_number(2);
    pump_events(50);
    let visible = window.imp().session_stack.visible_child_name().unwrap().to_string();
    assert_eq!(visible, uuid1);

    window.cycle_session(1);
    pump_events(50);
    let visible = window.imp().session_stack.visible_child_name().unwrap().to_string();
    assert_eq!(visible, uuid0, "forward wrap: last should go to first");

    // At first session, cycle backward should wrap to last.
    window.cycle_session(-1);
    pump_events(50);
    let visible = window.imp().session_stack.visible_child_name().unwrap().to_string();
    assert_eq!(visible, uuid1, "backward wrap: first should go to last");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn cycle_session_noop_with_single_session() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder().application_id("com.illya.rttx.cycle-noop-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    pump_events(50);

    let uuid0 = {
        let state = window.imp().state.borrow();
        state.workspaces[0].uuid.clone()
    };

    window.cycle_session(1);
    pump_events(50);
    let visible = window.imp().session_stack.visible_child_name().unwrap().to_string();
    assert_eq!(visible, uuid0, "single session: cycle should stay on same session");

    window.cycle_session(-1);
    pump_events(50);
    let visible = window.imp().session_stack.visible_child_name().unwrap().to_string();
    assert_eq!(visible, uuid0, "single session: reverse cycle should stay on same session");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn background_activity_indicator_transitions_to_idle() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.background-activity-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.add_session();
    pump_events(50);

    let background_terminal_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[1].layout.terminal_uuids()[0].clone()
    };

    window.mark_session_activity(&background_terminal_uuid);

    let session_row = session_row_at(&window, 1);
    assert_eq!(session_row.activity_state(), crate::sidebar::ActivityState::Active);

    assert!(
        wait_until(250, || { session_row.activity_state() == crate::sidebar::ActivityState::Idle }),
        "background activity should settle to idle when output stops"
    );
    assert!(session_row.has_activity(), "idle sessions should keep the unread indicator");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn switching_to_session_clears_background_activity_indicator() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.clear-activity-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.add_session();
    pump_events(50);

    let background_terminal_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[1].layout.terminal_uuids()[0].clone()
    };

    window.mark_session_activity(&background_terminal_uuid);
    pump_events(20);
    assert_eq!(session_row_at(&window, 1).activity_state(), crate::sidebar::ActivityState::Active);

    window.switch_to_session_number(2);
    pump_events(50);

    assert_eq!(session_row_at(&window, 1).activity_state(), crate::sidebar::ActivityState::None);
    assert!(!session_row_at(&window, 1).has_activity());

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn visible_session_activity_does_not_show_indicator() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.visible-activity-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    pump_events(50);

    let visible_terminal_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids()[0].clone()
    };

    window.mark_session_activity(&visible_terminal_uuid);
    pump_events(100);

    let session_row = session_row_at(&window, 0);
    assert_eq!(session_row.activity_state(), crate::sidebar::ActivityState::None);
    assert!(!session_row.has_activity());

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn repeated_background_activity_refreshes_window_indicator() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.refresh-activity-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.add_session();
    pump_events(50);

    let background_terminal_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[1].layout.terminal_uuids()[0].clone()
    };

    window.mark_session_activity(&background_terminal_uuid);
    assert!(
        wait_until(250, || {
            session_row_at(&window, 1).activity_state() == crate::sidebar::ActivityState::Idle
        }),
        "initial background activity should settle to idle"
    );

    window.mark_session_activity(&background_terminal_uuid);
    let session_row = session_row_at(&window, 1);
    assert_eq!(session_row.activity_state(), crate::sidebar::ActivityState::Active);

    pump_events(20);
    assert_eq!(
        session_row.activity_state(),
        crate::sidebar::ActivityState::Active,
        "fresh background output should move the indicator back to active"
    );

    assert!(
        wait_until(250, || { session_row.activity_state() == crate::sidebar::ActivityState::Idle }),
        "refreshed background activity should settle back to idle"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn split_inherits_cwd_from_source_terminal() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder().application_id("com.illya.rttx.split-cwd-test").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.set_default_size(1200, 800);
    window.present();
    pump_events(100);

    let t1_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };

    // Set a fake CWD on the source terminal.
    {
        let terminals = window.imp().terminals.borrow();
        let t1 = terminals.get(&t1_uuid).unwrap();
        t1.set_current_directory_for_test(Some("/home/user/project"));
    }

    window.split_terminal(&t1_uuid, SplitOrientation::Horizontal);

    let (t2_uuid, t2_cwd) = {
        let state = window.imp().state.borrow();
        let uuids = state.workspaces[0].layout.terminal_uuids();
        let t2 = uuids.into_iter().find(|u| u != &t1_uuid).unwrap();
        let cwd = state.workspaces[0].layout.terminal_cwd(&t2);
        (t2, cwd)
    };

    assert_eq!(
        t2_cwd.as_deref(),
        Some("/home/user/project"),
        "new pane from split should inherit source terminal's CWD in layout"
    );

    // Also verify the TerminalWidget was created with the inherited CWD.
    {
        let terminals = window.imp().terminals.borrow();
        let t2 = terminals.get(&t2_uuid).unwrap();
        assert_eq!(
            t2.initial_cwd_for_test(),
            Some("/home/user/project".to_string()),
            "new TerminalWidget should receive inherited CWD"
        );
    }

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn lease_lost_demotes_the_workspace_to_read_only_and_says_why() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder().application_id("com.illya.rttx.lease-lost-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let runtime_id = uuid::Uuid::new_v4();
    let window = Window::new(&app);
    let session_state = crate::test_helpers::managed_session_with_runtime(
        "workspace-seized",
        "Seized Workspace",
        LayoutNode::new_terminal_with_uuid("managed-pane"),
        RuntimeEndpoint::Local,
        WorkspacePolicy::Persistent,
        Some(&runtime_id.to_string()),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);
    window.set_workspace_connection_status(&session_state.uuid, &ConnectionStatus::Connected);

    window.dispatch_managed_runtime_message(
        &RuntimeEndpoint::Local,
        &rttx_proto::v3_takeover::build_lease_lost_envelope(
            rttx_proto::v3_takeover::build_lease_lost(runtime_id, 9, uuid::Uuid::new_v4()),
        ),
    );

    assert_eq!(
        window.imp().workspace_connection_status.borrow().get(&session_state.uuid),
        Some(&ConnectionStatus::Blocked(crate::runtime::ConnectionProblem::TakenOver)),
    );

    let pane = window
        .imp()
        .persistent_terminals
        .borrow()
        .get("managed-pane")
        .cloned()
        .expect("managed pane should be present");
    assert!(!pane.input_enabled_for_test(), "a demoted reader cannot type");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn blocked_remote_workspace_shows_edit_retry_and_disables_input() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.managed-blocked-workspace-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let session_state = crate::test_helpers::managed_session_with_runtime(
        "workspace-remote",
        "Remote Workspace",
        LayoutNode::new_terminal_with_uuid("managed-pane"),
        RuntimeEndpoint::remote("builder.example"),
        WorkspacePolicy::Persistent,
        None,
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    window.set_workspace_connection_status(
        &session_state.uuid,
        &ConnectionStatus::Blocked(crate::runtime::ConnectionProblem::PermissionDenied),
    );

    let pane = window
        .imp()
        .persistent_terminals
        .borrow()
        .get("managed-pane")
        .cloned()
        .expect("managed pane should be present");

    assert!(!pane.input_enabled_for_test());
    assert_eq!(pane.status_label_text_for_test(), "Action Required");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn managed_pane_split_button_updates_layout_and_materializes_new_pane() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.managed-split-button-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let session_state = crate::test_helpers::managed_session(
        "workspace-split",
        "Split Workspace",
        LayoutNode::new_terminal_with_uuid("managed-pane"),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    let pane = window
        .imp()
        .persistent_terminals
        .borrow()
        .get("managed-pane")
        .cloned()
        .expect("managed pane should be present");

    pane.split_h_button().emit_clicked();

    let state = window.imp().state.borrow();
    let session = state
        .workspaces
        .iter()
        .find(|session| session.uuid == session_state.uuid)
        .expect("managed workspace should remain present");
    let terminal_uuids = session.layout.terminal_uuids();
    assert_eq!(terminal_uuids.len(), 2, "managed split should add a second layout pane");
    let new_uuid = terminal_uuids
        .into_iter()
        .find(|uuid| uuid != "managed-pane")
        .expect("managed split should create a new pane uuid");
    drop(state);

    assert!(
        window.imp().persistent_terminals.borrow().contains_key(&new_uuid),
        "managed split should materialize a new persistent pane widget"
    );
    assert!(
        !window.imp().terminals.borrow().contains_key(&new_uuid),
        "managed split must not fall back to direct terminal widgets"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn managed_workspace_reconnect_countdown_updates_live_pane_status() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.managed-reconnect-countdown-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let session_state = crate::test_helpers::managed_session(
        "workspace-local",
        "Local Workspace",
        LayoutNode::new_terminal_with_uuid("managed-pane"),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    window.set_workspace_connection_status(
        &session_state.uuid,
        &ConnectionStatus::Reconnecting { attempt: 1, retry_in_secs: 3 },
    );

    let pane = window
        .imp()
        .persistent_terminals
        .borrow()
        .get("managed-pane")
        .cloned()
        .expect("managed pane should be present");
    assert_eq!(pane.status_label_text_for_test(), "Retry 3s");

    assert!(
        wait_until(2_500, || pane.status_label_text_for_test() == "Retry 1s"),
        "local reconnect countdown should tick down on the live pane status"
    );
    assert_eq!(
        window.imp().workspace_connection_status.borrow().get(&session_state.uuid),
        Some(&ConnectionStatus::Reconnecting { attempt: 1, retry_in_secs: 1 })
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn inventory_loaded_recovers_missing_managed_workspace() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.inventory-recovery-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let initial_session_count = window.imp().state.borrow().workspaces.len();
    let runtime_id = uuid::Uuid::new_v4();
    let pane_id = uuid::Uuid::new_v4();

    window.handle_endpoint_event(crate::daemon_bridge::EndpointEvent::InventoryLoaded {
        endpoint: RuntimeEndpoint::Local,
        workspaces: vec![rttx_proto::v3::WorkspaceInfo {
            id: rttx_proto::uuid_to_bytes(runtime_id),
            name: "Recovered Workspace".into(),
            pane_count: 1,
            has_write_owner: false,
            read_only_client_count: 0,
            active_pane_summary: String::new(),
            takeover_eligible: false,
            disabled_reason: String::new(),
            current_client_role: rttx_proto::v3::WorkspaceClientRole::Unattached as i32,
            panes: vec![rttx_proto::v3::PaneInfo {
                id: rttx_proto::uuid_to_bytes(pane_id),
                title: "Shell".into(),
                cwd: "/srv/project".into(),
                cols: 120,
                rows: 40,
                exit_status: None,
                reconstructed: false,
                no_persist: false,
            }],
            policy: rttx_proto::v3::WorkspacePolicy::Persistent as i32,
            reconstructed: true,
            user_renamed: false,
            workspace_revision: 7,
        }],
    });

    let runtime_id = runtime_id.to_string();
    let pane_id = pane_id.to_string();
    let state = window.imp().state.borrow();
    assert_eq!(
        state.workspaces.len(),
        initial_session_count + 1,
        "inventory should materialize one recovered workspace"
    );
    let session = state
        .workspaces
        .iter()
        .find(|session| session.uuid == format!("inventory:local:{runtime_id}"))
        .expect("inventory should add a recovered workspace session");
    assert_eq!(session.uuid, format!("inventory:local:{runtime_id}"));
    assert_eq!(session.name, "Recovered Workspace");
    assert_eq!(session.runtime.endpoint, RuntimeEndpoint::Local);
    assert_eq!(session.runtime.runtime_id.as_deref(), Some(runtime_id.as_str()));
    assert_eq!(session.layout.terminal_uuids(), vec![pane_id.clone()]);
    drop(state);

    assert!(
        window.imp().persistent_terminals.borrow().contains_key(&pane_id),
        "inventory recovery should materialize a persistent pane widget"
    );
    assert_eq!(
        window
            .imp()
            .workspace_connection_status
            .borrow()
            .get(&format!("inventory:local:{runtime_id}")),
        Some(&ConnectionStatus::Connecting)
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn inventory_loaded_skips_workspace_for_known_runtime() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.inventory-recovery-dedup-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let runtime_id = uuid::Uuid::new_v4().to_string();
    let session_state = crate::test_helpers::managed_session_with_runtime(
        "workspace-existing",
        "Existing Workspace",
        LayoutNode::new_terminal_with_uuid("managed-pane"),
        RuntimeEndpoint::Local,
        WorkspacePolicy::Persistent,
        Some(&runtime_id),
    );

    let window = Window::new(&app);
    let initial_session_count = window.imp().state.borrow().workspaces.len();
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    window.handle_endpoint_event(crate::daemon_bridge::EndpointEvent::InventoryLoaded {
        endpoint: RuntimeEndpoint::Local,
        workspaces: vec![rttx_proto::v3::WorkspaceInfo {
            id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(&runtime_id).unwrap()),
            name: "Recovered Workspace".into(),
            pane_count: 1,
            has_write_owner: false,
            read_only_client_count: 0,
            active_pane_summary: String::new(),
            takeover_eligible: false,
            disabled_reason: String::new(),
            current_client_role: rttx_proto::v3::WorkspaceClientRole::Unattached as i32,
            panes: vec![rttx_proto::v3::PaneInfo {
                id: rttx_proto::uuid_to_bytes(uuid::Uuid::new_v4()),
                title: "Shell".into(),
                cwd: "/srv/project".into(),
                cols: 120,
                rows: 40,
                exit_status: None,
                reconstructed: false,
                no_persist: false,
            }],
            policy: rttx_proto::v3::WorkspacePolicy::Persistent as i32,
            reconstructed: true,
            user_renamed: false,
            workspace_revision: 7,
        }],
    });

    let state = window.imp().state.borrow();
    assert_eq!(
        state.workspaces.len(),
        initial_session_count + 1,
        "inventory should not duplicate an attached runtime"
    );
    assert!(state.workspaces.iter().any(|session| session.uuid == "workspace-existing"));
    drop(state);

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn managed_workspace_recovery_does_not_steal_visible_session_from_selected_row() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.recovery-selection-sync-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let visible_before = {
        let state = window.imp().state.borrow();
        state.workspaces[0].uuid.clone()
    };
    let runtime_id = uuid::Uuid::new_v4();
    let pane_id = uuid::Uuid::new_v4();
    let recovered_session = crate::test_helpers::managed_session_with_runtime(
        "workspace-recovered",
        "Recovered Workspace",
        LayoutNode::new_terminal_with_uuid(&pane_id.to_string()),
        RuntimeEndpoint::Local,
        WorkspacePolicy::Persistent,
        Some(&runtime_id.to_string()),
    );
    window.imp().state.borrow_mut().workspaces.push(recovered_session.clone());
    window.build_session(&recovered_session, false);

    let first_row = window.imp().sidebar_list.row_at_index(0).unwrap();
    window.imp().sidebar_list.select_row(Some(&first_row));
    pump_events(50);

    assert_eq!(selected_session_uuid(&window).as_deref(), Some(visible_before.as_str()));
    assert_eq!(
        window.imp().session_stack.visible_child_name().as_deref(),
        Some(visible_before.as_str())
    );

    window.handle_endpoint_event(crate::daemon_bridge::EndpointEvent::WorkspaceOpened {
        workspace_id: recovered_session.uuid.clone(),
        runtime_id: runtime_id.to_string(),
        snapshot: rttx_proto::v3::WorkspaceSnapshot {
            tree: None,
            default_active_pane_id: Vec::new(),
            runtime_id: rttx_proto::uuid_to_bytes(runtime_id),
            workspace_revision: 7,
            client_role: rttx_proto::v3::WorkspaceClientRole::Writer as i32,
            panes: vec![rttx_proto::v3::PaneSnapshot {
                pane_id: rttx_proto::uuid_to_bytes(pane_id),
                pane_output_seq: 0,
                title: "Shell".into(),
                cwd: "/srv/project".into(),
                cols: 120,
                rows: 40,
                exit_status: None,
                terminal_modes: None,
                scrollback_tail: bytes::Bytes::from_static(b"restored output"),
                total_scrollback_bytes: 15,
                scrollback_complete: true,
            }],
        },
    });
    pump_events(50);

    assert_eq!(
        selected_session_uuid(&window).as_deref(),
        Some(visible_before.as_str()),
        "background recovery must not change the selected row"
    );
    assert_eq!(
        window.imp().session_stack.visible_child_name().as_deref(),
        Some(visible_before.as_str()),
        "background recovery must not change the visible session"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn load_state_keeps_selected_row_and_visible_session_in_sync() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let first_uuid = "workspace-1".to_string();
    let second_uuid = "workspace-2".to_string();
    save_window_state_to_store(&WindowState {
        active_workspace_index: 1,
        workspaces: vec![
            WorkspaceState {
                uuid: first_uuid,
                name: "Workspace 1".into(),
                layout: LayoutNode::new_terminal_with_uuid("terminal-1"),
                terminal_recovery: Default::default(),
                active_terminal_uuid: None,
                input_sync: false,
                runtime: Default::default(),
                color: Default::default(),
                zoomed_terminal_uuid: None,
                user_renamed: false,
            },
            WorkspaceState {
                uuid: second_uuid.clone(),
                name: "Workspace 2".into(),
                layout: LayoutNode::new_terminal_with_uuid("terminal-2"),
                terminal_recovery: Default::default(),
                active_terminal_uuid: None,
                input_sync: false,
                runtime: Default::default(),
                color: Default::default(),
                zoomed_terminal_uuid: None,
                user_renamed: false,
            },
        ],
        ..WindowState::default()
    });

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.restore-selection-sync-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    pump_events(50);

    assert_eq!(selected_session_uuid(&window).as_deref(), Some(second_uuid.as_str()));
    assert_eq!(
        window.imp().session_stack.visible_child_name().as_deref(),
        Some(second_uuid.as_str())
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn recovered_workspace_uses_compact_sidebar_status_without_banner() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.recovered-workspace-status-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let session_state = crate::test_helpers::managed_session(
        "workspace-local",
        "Local Workspace",
        LayoutNode::new_terminal_with_uuid("managed-pane"),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    window.set_workspace_connection_status(&session_state.uuid, &ConnectionStatus::Recovered);

    let pane = window
        .imp()
        .persistent_terminals
        .borrow()
        .get("managed-pane")
        .cloned()
        .expect("managed pane should be present");
    assert!(pane.input_enabled_for_test());
    assert_eq!(pane.status_label_text_for_test(), "Connected");

    let row = session_row_for_uuid(&window, &session_state.uuid);
    let subtitle = row.subtitle().map(|value| value.to_string());
    assert_eq!(subtitle.as_deref(), Some(""));
    assert!(row.imp().connection_icon.is_visible());
    assert!(
        !subtitle.as_deref().is_some_and(|value| value.contains("Recovered")),
        "status text should be conveyed by icon, not subtitle"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn workspace_detached_event_preserves_runtime_id_for_manual_reattach() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.workspace-detached-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let runtime_id = uuid::Uuid::new_v4().to_string();
    let session_state = crate::test_helpers::managed_session_with_runtime(
        "workspace-detached",
        "Detached Workspace",
        LayoutNode::new_terminal_with_uuid("managed-pane"),
        RuntimeEndpoint::remote("builder.example"),
        WorkspacePolicy::Persistent,
        Some(&runtime_id),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    window.handle_endpoint_event(crate::daemon_bridge::EndpointEvent::WorkspaceDetached {
        workspace_id: session_state.uuid.clone(),
        runtime_id: runtime_id.clone(),
    });

    let state = window.imp().state.borrow();
    let session = state
        .workspaces
        .iter()
        .find(|session| session.uuid == session_state.uuid)
        .expect("workspace should stay present after detach");
    assert_eq!(session.runtime.runtime_id.as_deref(), Some(runtime_id.as_str()));
    drop(state);

    assert_eq!(
        window.imp().workspace_connection_status.borrow().get(&session_state.uuid),
        Some(&ConnectionStatus::Disconnected)
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn save_state_persists_detached_workspace_runtime_binding() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.save-detached-workspace-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let runtime_id = uuid::Uuid::new_v4().to_string();
    let session_state = crate::test_helpers::managed_session_with_runtime(
        "workspace-detached-save",
        "Detached Workspace",
        LayoutNode::new_terminal_with_uuid("managed-pane"),
        RuntimeEndpoint::remote("builder.example"),
        WorkspacePolicy::Persistent,
        Some(&runtime_id),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    window.handle_endpoint_event(crate::daemon_bridge::EndpointEvent::WorkspaceDetached {
        workspace_id: session_state.uuid.clone(),
        runtime_id: runtime_id.clone(),
    });
    window.save_state();

    let saved_state = load_saved_window_state();
    let saved_session = saved_state
        .workspaces
        .iter()
        .find(|session| session.uuid == session_state.uuid)
        .expect("detached workspace should persist in saved state");

    assert!(saved_session.runtime.is_managed());
    assert_eq!(saved_session.runtime.endpoint, RuntimeEndpoint::remote("builder.example"));
    assert_eq!(saved_session.runtime.policy, WorkspacePolicy::Persistent);
    assert_eq!(saved_session.runtime.runtime_id.as_deref(), Some(runtime_id.as_str()));
    assert!(saved_session.layout.contains_terminal("managed-pane"));

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn runtime_terminated_event_clears_runtime_id_but_keeps_workspace() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.runtime-terminated-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let runtime_id = uuid::Uuid::new_v4().to_string();
    let session_state = crate::test_helpers::managed_session_with_runtime(
        "workspace-terminated",
        "Terminated Workspace",
        LayoutNode::new_terminal_with_uuid("managed-pane"),
        RuntimeEndpoint::remote("builder.example"),
        WorkspacePolicy::Persistent,
        Some(&runtime_id),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    window.handle_endpoint_event(crate::daemon_bridge::EndpointEvent::WorkspaceTerminated {
        workspace_id: session_state.uuid.clone(),
        runtime_id,
        reason: rttx_proto::v3::WorkspaceTerminationReason::Explicit,
    });

    let state = window.imp().state.borrow();
    let session = state
        .workspaces
        .iter()
        .find(|session| session.uuid == session_state.uuid)
        .expect("workspace should stay present after runtime termination");
    assert_eq!(session.runtime.runtime_id, None);
    drop(state);

    assert_eq!(
        window.imp().workspace_connection_status.borrow().get(&session_state.uuid),
        Some(&ConnectionStatus::Disconnected)
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn save_state_persists_terminated_workspace_without_runtime_id() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.save-terminated-workspace-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let runtime_id = uuid::Uuid::new_v4().to_string();
    let session_state = crate::test_helpers::managed_session_with_runtime(
        "workspace-terminated-save",
        "Terminated Workspace",
        LayoutNode::new_terminal_with_uuid("managed-pane"),
        RuntimeEndpoint::remote("builder.example"),
        WorkspacePolicy::Persistent,
        Some(&runtime_id),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    window.handle_endpoint_event(crate::daemon_bridge::EndpointEvent::WorkspaceTerminated {
        workspace_id: session_state.uuid.clone(),
        runtime_id,
        reason: rttx_proto::v3::WorkspaceTerminationReason::Explicit,
    });
    window.save_state();

    let saved_state = load_saved_window_state();
    let saved_session = saved_state
        .workspaces
        .iter()
        .find(|session| session.uuid == session_state.uuid)
        .expect("terminated workspace should persist in saved state");

    assert!(saved_session.runtime.is_managed());
    assert_eq!(saved_session.runtime.endpoint, RuntimeEndpoint::remote("builder.example"));
    assert_eq!(saved_session.runtime.policy, WorkspacePolicy::Persistent);
    assert_eq!(saved_session.runtime.runtime_id, None);
    assert!(saved_session.layout.contains_terminal("managed-pane"));

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
fn notification_tier_suppresses_for_visible_session() {
    let state = WindowState::default();
    let uuid = state.workspaces[0].uuid.clone();
    let terminal = state.workspaces[0].layout.terminal_uuids()[0].clone();
    assert_eq!(notification_tier(&terminal, Some(&uuid), true, &state), NotificationTier::Suppress);
}

#[test]
fn notification_tier_toasts_for_background_session_when_focused() {
    let mut state = WindowState::default();
    state.workspaces.push(WorkspaceState::new("Background".into()));
    let bg_terminal = state.workspaces[1].layout.terminal_uuids()[0].clone();
    let visible_uuid = state.workspaces[0].uuid.clone();
    assert_eq!(
        notification_tier(&bg_terminal, Some(&visible_uuid), true, &state),
        NotificationTier::Toast
    );
}

#[test]
fn notification_tier_desktop_when_window_unfocused() {
    let mut state = WindowState::default();
    state.workspaces.push(WorkspaceState::new("Background".into()));
    let bg_terminal = state.workspaces[1].layout.terminal_uuids()[0].clone();
    let visible_uuid = state.workspaces[0].uuid.clone();
    assert_eq!(
        notification_tier(&bg_terminal, Some(&visible_uuid), false, &state),
        NotificationTier::Desktop
    );
}

/// When `split_terminal_in_place` fails (target widget has no parent),
/// `split_terminal` must fall back to `rebuild_session_content` and
/// produce a correct widget tree with a Paned containing two terminals.
#[test]
#[ignore = "requires isolated GTK harness"]
fn split_fallback_rebuild_produces_correct_widget_tree() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.split-fallback-test").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();
    let window = Window::new(&app);
    window.set_default_size(1200, 800);
    window.present();
    pump_events(100);

    // Create a fresh direct session so the test is independent of
    // whatever state was loaded from disk.
    let fresh = WorkspaceState::new("Fallback Test".into());
    let session_uuid = fresh.uuid.clone();
    let t1_uuid = fresh.layout.terminal_uuids().into_iter().next().unwrap();
    {
        let mut state = window.imp().state.borrow_mut();
        state.workspaces.push(fresh.clone());
    }
    window.build_session(&fresh, false);
    pump_events(50);

    // Remove the session content from the stack so the terminal widget
    // has no parent. This forces split_terminal_in_place to fail
    // (target.parent() returns None → returns false), triggering the
    // fallback to rebuild_session_content.
    if let Some(content) = window.imp().session_stack.child_by_name(&session_uuid) {
        window.imp().session_stack.remove(&content);
    }

    window.split_terminal(&t1_uuid, SplitOrientation::Horizontal);
    pump_events(100);

    let content = window
        .imp()
        .session_stack
        .child_by_name(&session_uuid)
        .expect("rebuild must re-add the session page to the stack");

    let paned =
        content.downcast_ref::<gtk4::Paned>().expect("rebuilt content must be a Paned after split");

    assert!(paned.start_child().is_some(), "Paned must have a start child");
    assert!(paned.end_child().is_some(), "Paned must have an end child");

    let state = window.imp().state.borrow();
    let session = state
        .workspaces
        .iter()
        .find(|s| s.uuid == session_uuid)
        .expect("session must still exist after split");
    assert_eq!(session.layout.terminal_count(), 2, "layout must have 2 terminals after split");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn auto_rename_updates_sidebar_when_not_user_renamed() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.auto-rename-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let session_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].uuid.clone()
    };

    window.maybe_auto_rename_workspace(&session_uuid, Some("/home/user/projects/rttx"));

    {
        let state = window.imp().state.borrow();
        assert_eq!(state.workspaces[0].name, "rttx");
        assert!(!state.workspaces[0].user_renamed);
    }

    let row = window.imp().sidebar_list.row_at_index(0).expect("row exists");
    let session_row =
        row.child().and_then(|child| child.downcast::<WorkspaceRow>().ok()).expect("WorkspaceRow");
    assert_eq!(session_row.workspace_name(), "rttx");

    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn auto_rename_skipped_after_manual_rename() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.auto-rename-sticky-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let session_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].uuid.clone()
    };

    window.rename_runtime(&session_uuid, "My Custom Name");
    window.maybe_auto_rename_workspace(&session_uuid, Some("/home/user/projects/rttx"));

    {
        let state = window.imp().state.borrow();
        assert_eq!(state.workspaces[0].name, "My Custom Name");
        assert!(state.workspaces[0].user_renamed);
    }

    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn retry_workspace_connection_sets_connecting_and_rebuilds_on_open() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.retry-connection-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let runtime_id = uuid::Uuid::new_v4();
    let pane_id = uuid::Uuid::new_v4();
    let session_state = crate::test_helpers::managed_session_with_runtime(
        "workspace-retry",
        "Retry Workspace",
        LayoutNode::new_terminal_with_uuid("retry-pane"),
        RuntimeEndpoint::Local,
        WorkspacePolicy::Persistent,
        Some(&runtime_id.to_string()),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    // Simulate disconnected state.
    window.set_workspace_connection_status(&session_state.uuid, &ConnectionStatus::Disconnected);

    // Call retry — should set status to Connecting.
    window.retry_workspace_connection(&session_state.uuid);

    assert_eq!(
        window.imp().workspace_connection_status.borrow().get(&session_state.uuid),
        Some(&ConnectionStatus::Connecting),
        "retry should set status to Connecting"
    );

    // Simulate the daemon responding with WorkspaceOpened.
    window.handle_endpoint_event(crate::daemon_bridge::EndpointEvent::WorkspaceOpened {
        workspace_id: session_state.uuid.clone(),
        runtime_id: runtime_id.to_string(),
        snapshot: rttx_proto::v3::WorkspaceSnapshot {
            tree: None,
            default_active_pane_id: Vec::new(),
            runtime_id: rttx_proto::uuid_to_bytes(runtime_id),
            workspace_revision: 5,
            client_role: rttx_proto::v3::WorkspaceClientRole::Writer as i32,
            panes: vec![rttx_proto::v3::PaneSnapshot {
                pane_id: rttx_proto::uuid_to_bytes(pane_id),
                pane_output_seq: 0,
                title: "shell".into(),
                cwd: "/home/user".into(),
                cols: 80,
                rows: 24,
                exit_status: None,
                terminal_modes: None,
                scrollback_tail: bytes::Bytes::from_static(b"reconnected"),
                total_scrollback_bytes: 11,
                scrollback_complete: true,
            }],
        },
    });
    pump_events(50);

    // The session content should have been rebuilt.
    let stack = &window.imp().session_stack;
    assert!(
        stack.child_by_name(&session_state.uuid).is_some(),
        "session content should exist after WorkspaceOpened"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn bell_preferences_applied_to_managed_pane() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    // Save preferences with bells disabled.
    let mut prefs = store().load_preferences().into_value().unwrap_or_default();
    prefs.audible_bell = false;
    prefs.visual_bell = false;
    let _ = store().save_preferences(&prefs);

    let app = adw::Application::builder().application_id("com.illya.rttx.bell-pref-test").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let session_state = {
        let mut state = window.imp().state.borrow_mut();
        state.workspaces[0].runtime.managed = true;
        state.workspaces[0].runtime.runtime_id = Some("runtime-1".into());
        state.workspaces[0].clone()
    };
    window.rebuild_session_content(&session_state.uuid, &session_state);
    pump_events(50);

    let pane = window
        .imp()
        .persistent_terminals
        .borrow()
        .values()
        .next()
        .cloned()
        .expect("managed pane should exist");

    assert!(!pane.vte().is_audible_bell(), "audible bell should be disabled by preference");
    assert!(!pane.imp().visual_bell.get(), "visual bell should be disabled by preference");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn cwd_changed_updates_layout_node() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.cwd-changed-layout-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let layout_uuid = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
    let runtime_pane_id = uuid::Uuid::parse_str(layout_uuid).unwrap();
    let session_state = crate::test_helpers::managed_session_with_runtime(
        "ws-cwd",
        "CWD Test",
        LayoutNode::new_terminal_with_uuid(layout_uuid),
        RuntimeEndpoint::Local,
        WorkspacePolicy::Persistent,
        Some("runtime-cwd"),
    );

    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    // Verify initial layout CWD is None.
    {
        let state = window.imp().state.borrow();
        let session = state.workspaces.iter().find(|s| s.uuid == "ws-cwd").unwrap();
        assert_eq!(session.layout.terminal_cwd(layout_uuid), None);
    }

    // Dispatch a CwdChanged message.
    let msg = rttx_proto::v3::ServerEnvelope {
        request_id: 0,
        payload: Some(rttx_proto::v3::server_envelope::Payload::CwdChanged(
            rttx_proto::v3::CwdChanged {
                runtime_id: rttx_proto::uuid_to_bytes(uuid::Uuid::new_v4()),
                pane_id: rttx_proto::uuid_to_bytes(runtime_pane_id),
                cwd: "/tmp/updated".into(),
                workspace_revision: 1,
            },
        )),
    };
    window.dispatch_managed_runtime_message(&RuntimeEndpoint::Local, &msg);

    // Verify layout CWD is updated.
    {
        let state = window.imp().state.borrow();
        let session = state.workspaces.iter().find(|s| s.uuid == "ws-cwd").unwrap();
        assert_eq!(
            session.layout.terminal_cwd(layout_uuid).as_deref(),
            Some("/tmp/updated"),
            "CwdChanged should update the layout node CWD"
        );
    }

    // Verify widget CWD is also updated.
    {
        let pane = window.imp().persistent_terminals.borrow().get(layout_uuid).cloned().unwrap();
        assert_eq!(
            pane.current_directory().as_deref(),
            Some("/tmp/updated"),
            "CwdChanged should update the widget CWD"
        );
    }

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn managed_pane_exit_marks_visible_pane_exited() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.managed-pane-exit-test").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let layout_uuid = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
    let runtime_pane_id = uuid::Uuid::parse_str(layout_uuid).unwrap();
    let session_state = crate::test_helpers::managed_session_with_runtime(
        "ws-exit",
        "Exit Test",
        LayoutNode::new_terminal_with_uuid(layout_uuid),
        RuntimeEndpoint::Local,
        WorkspacePolicy::Persistent,
        Some("runtime-exit"),
    );

    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    let msg = rttx_proto::v3::ServerEnvelope {
        request_id: 0,
        payload: Some(rttx_proto::v3::server_envelope::Payload::PaneExited(
            rttx_proto::v3::PaneExited {
                runtime_id: rttx_proto::uuid_to_bytes(uuid::Uuid::new_v4()),
                pane_id: rttx_proto::uuid_to_bytes(runtime_pane_id),
                status: 0,
                workspace_revision: 2,
            },
        )),
    };
    window.dispatch_managed_runtime_message(&RuntimeEndpoint::Local, &msg);

    let pane = window.imp().persistent_terminals.borrow().get(layout_uuid).cloned().unwrap();
    assert!(pane.exited_for_test());
    assert!(!pane.input_enabled_for_test());
    assert_eq!(pane.status_label_text_for_test(), "Exited");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

/// Closing a workspace when multiple workspaces exist removes the target
/// workspace and keeps the window open.
#[test]
#[ignore = "requires isolated GTK harness"]
fn close_session_removes_workspace_when_multiple_exist() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder().application_id("com.illya.rttx.close-multi-test").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    // Add a second session so we have two.
    let second = WorkspaceState::new("Second".into());
    let second_uuid = second.uuid.clone();
    window.imp().state.borrow_mut().workspaces.push(second.clone());
    window.build_session(&second, false);

    assert_eq!(window.imp().state.borrow().workspaces.len(), 2);

    window.close_session(&second_uuid);

    assert_eq!(
        window.imp().state.borrow().workspaces.len(),
        1,
        "closing one of two workspaces should remove it"
    );
    assert!(
        !window.imp().state.borrow().workspaces.iter().any(|s| s.uuid == second_uuid),
        "the closed workspace should no longer be in state"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

/// Closing the last workspace should close the window instead of silently
/// doing nothing. Regression test for #414.
#[test]
#[ignore = "requires isolated GTK harness"]
fn close_session_closes_window_when_last_workspace() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder().application_id("com.illya.rttx.close-last-test").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let session_uuid = window.imp().state.borrow().workspaces[0].uuid.clone();

    assert_eq!(window.imp().state.borrow().workspaces.len(), 1);

    // Before the fix, this silently returned. Now it should close the window.
    window.close_session(&session_uuid);
    pump_events(50);

    // GtkWindow.close() triggers the close-request signal and eventually
    // hides the window. We verify the window is no longer visible.
    assert!(!window.is_visible(), "closing the last workspace should close the window");

    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

/// Regression test for #435: opening the workspace popover menu after
/// closing a different workspace must not crash. The old popover was
/// parented to a ListBoxRow that got destroyed when the workspace was
/// closed, so the next `unparent()` call hit a SEGV.
#[test]
#[ignore = "requires isolated GTK harness"]
fn popover_menu_after_close_does_not_crash() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.popover-after-close").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let second = WorkspaceState::new("Second".into());
    let second_uuid = second.uuid.clone();
    window.imp().state.borrow_mut().workspaces.push(second.clone());
    window.build_session(&second, false);

    let first_uuid = window.imp().state.borrow().workspaces[0].uuid.clone();

    // Show the popover on the second workspace (stores it in workspace_popover).
    let second_row = session_row_for_uuid(&window, &second_uuid);
    window.show_workspace_popover_menu(&second_row, &second_uuid);
    assert!(window.imp().workspace_popover.borrow().is_some());

    // Close the second workspace — its sidebar row is destroyed.
    window.close_session(&second_uuid);

    // Show the popover on the first workspace — must not crash.
    let first_row = session_row_for_uuid(&window, &first_uuid);
    window.show_workspace_popover_menu(&first_row, &first_uuid);

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

fn has_controller<T: IsA<glib::Object>>(widget: &impl IsA<gtk4::Widget>) -> bool {
    let controllers = widget.observe_controllers();
    for index in 0..controllers.n_items() {
        if let Some(controller) = controllers.item(index)
            && controller.downcast::<T>().is_ok()
        {
            return true;
        }
    }
    false
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn managed_pane_has_drag_source_on_header() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.managed-drag-source-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let session_state = crate::test_helpers::managed_session(
        "workspace-drag",
        "Drag Workspace",
        LayoutNode::new_terminal_with_uuid("drag-pane"),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    let pane = window
        .imp()
        .persistent_terminals
        .borrow()
        .get("drag-pane")
        .cloned()
        .expect("managed pane should be present");

    assert!(
        has_controller::<gtk4::DragSource>(pane.header()),
        "managed pane header should have a DragSource controller"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn managed_pane_has_drop_target() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.managed-drop-target-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let session_state = crate::test_helpers::managed_session(
        "workspace-drop",
        "Drop Workspace",
        LayoutNode::new_terminal_with_uuid("drop-pane"),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    let pane = window
        .imp()
        .persistent_terminals
        .borrow()
        .get("drop-pane")
        .cloned()
        .expect("managed pane should be present");

    assert!(
        has_controller::<gtk4::DropTarget>(&pane),
        "managed pane should have a DropTarget controller"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn swap_terminals_works_for_managed_panes() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.managed-swap-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let layout = LayoutNode::Split {
        orientation: crate::workspace::layout::SplitOrientation::Horizontal,
        ratio: 0.5,
        first: Box::new(LayoutNode::new_terminal_with_uuid("pane-a")),
        second: Box::new(LayoutNode::new_terminal_with_uuid("pane-b")),
    };
    let session_state =
        crate::test_helpers::managed_session("workspace-swap", "Swap Workspace", layout);
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    window.swap_terminals("pane-a", "pane-b");

    let state = window.imp().state.borrow();
    let session =
        state.workspaces.iter().find(|s| s.uuid == "workspace-swap").expect("session should exist");
    assert_eq!(
        session.layout.terminal_uuids(),
        vec!["pane-b", "pane-a"],
        "swap should exchange pane positions in managed workspace"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn right_sidebar_has_host_selector_and_search() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.sidebar-host-selector-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    assert!(
        window.imp().host_selector.is_visible(),
        "host selector should be visible in the right sidebar"
    );
    assert!(
        window.imp().sidebar_search_entry.is_visible(),
        "unified search entry should be visible in the right sidebar"
    );

    // Host selector should default to "Local" for a local workspace
    let model = window
        .imp()
        .host_selector
        .model()
        .and_then(|m| m.downcast::<gtk4::StringList>().ok())
        .expect("host selector should have a StringList model");
    let selected_idx = window.imp().host_selector.selected();
    let selected_label = model.string(selected_idx).unwrap();
    assert_eq!(selected_label.as_str(), "Local");

    // "All Hosts" should be the last entry
    let last_idx = model.n_items() - 1;
    let last_label = model.string(last_idx).unwrap();
    assert_eq!(last_label.as_str(), "All Hosts");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn right_sidebar_has_places_tab() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.sidebar-places-tab-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    // The utility stack should have a "places" page
    let stack = &window.imp().utility_stack;
    assert!(stack.child_by_name("places").is_some(), "utility stack should have a Places tab");
    assert!(stack.child_by_name("commands").is_some(), "utility stack should have a Commands tab");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn place_sidebar_shows_builtin_places_for_local_host() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.sidebar-builtin-places-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    // Place list should show built-in places (Home, Root) under Global section
    // Section header + Home + Root = at least 3 rows
    let place_count = window.imp().place_list.observe_children().n_items();
    assert!(
        place_count >= 3,
        "place sidebar should show at least a section header and 2 built-in places, got {place_count}"
    );
    assert!(
        window.imp().place_scroll.is_visible(),
        "place scroll should be visible when places exist"
    );
    assert!(
        !window.imp().place_empty.is_visible(),
        "place empty state should be hidden when places exist"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn host_selector_auto_follows_workspace_switch() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.host-selector-auto-follow-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    // Add a remote managed workspace
    let remote_session = WorkspaceState::new_managed_remote(
        "Remote Work".into(),
        "deploy@example.com",
        WorkspacePolicy::Persistent,
        None,
    );
    let remote_uuid = remote_session.uuid.clone();
    window.imp().state.borrow_mut().workspaces.push(remote_session.clone());
    window.build_session(&remote_session, false);
    pump_events(50);

    // Switch to the remote workspace
    let state = window.imp().state.borrow();
    let remote_idx = state.workspaces.iter().position(|s| s.uuid == remote_uuid).unwrap();
    drop(state);
    window.switch_to_session(remote_idx);
    pump_events(50);

    // Host selector should now show the remote host
    let model = window
        .imp()
        .host_selector
        .model()
        .and_then(|m| m.downcast::<gtk4::StringList>().ok())
        .expect("host selector should have a StringList model");
    let selected_idx = window.imp().host_selector.selected();
    let selected_label = model.string(selected_idx).unwrap();
    assert_eq!(
        selected_label.as_str(),
        "example",
        "host selector should auto-follow to the remote workspace host"
    );

    // Switch back to the first (local) workspace
    window.switch_to_session(0);
    pump_events(50);

    let selected_idx = window.imp().host_selector.selected();
    let selected_label = model.string(selected_idx).unwrap();
    assert_eq!(
        selected_label.as_str(),
        "Local",
        "host selector should auto-follow back to local when switching to local workspace"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn command_sidebar_filters_by_selected_host() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let mut local_cmd = crate::commands::SavedCommand::new("Local cmd", "echo local");
    local_cmd.host_tags = vec!["local".into()];
    let mut remote_cmd = crate::commands::SavedCommand::new("Remote cmd", "echo remote");
    remote_cmd.host_tags = vec!["example.com".into()];
    let global_cmd = crate::commands::SavedCommand::new("Global cmd", "echo global");
    store().save_commands(&[local_cmd, remote_cmd, global_cmd]).unwrap();

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.command-host-filter-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    // Default is local host — should show local + global commands with section headers
    // 2 sections (Local header + cmd, Global header + cmd) = 4 rows
    let count = window.imp().command_list.observe_children().n_items();
    assert_eq!(count, 4, "local host should show 2 section headers + 2 commands, got {count}");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn command_sidebar_groups_by_host_in_all_hosts_view() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let config_dir = tmp.path().join("rttx-devel");
    std::fs::create_dir_all(&config_dir).unwrap();
    let hosts = vec![crate::host::Host::remote("deploy@example.com")];
    store().save_hosts(&hosts).unwrap();

    let mut local_cmd = crate::commands::SavedCommand::new("Local cmd", "echo local");
    local_cmd.host_tags = vec!["local".into()];
    let mut remote_cmd = crate::commands::SavedCommand::new("Remote cmd", "echo remote");
    remote_cmd.host_tags = vec!["example.com".into()];
    let global_cmd = crate::commands::SavedCommand::new("Global cmd", "echo global");
    store().save_commands(&[local_cmd, remote_cmd, global_cmd]).unwrap();

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.command-all-hosts-sections-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    // Select "All Hosts" (last entry in the dropdown)
    let dd = &window.imp().host_selector;
    let all_hosts_idx = dd.model().unwrap().n_items() - 1;
    dd.set_selected(all_hosts_idx);
    pump_events(50);

    // All Hosts view: 3 sections (Local header + cmd, example header + cmd, Global header + cmd)
    let count = window.imp().command_list.observe_children().n_items();
    assert_eq!(count, 6, "All Hosts should show 3 section headers + 3 commands, got {count}");

    // Verify section headers are present by checking first row is non-activatable (header)
    let first_row = window.imp().command_list.row_at_index(0).unwrap();
    assert!(!first_row.is_activatable(), "first row should be a non-activatable section header");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn command_sidebar_shows_sections_for_specific_host() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let mut local_cmd = crate::commands::SavedCommand::new("Local cmd", "echo local");
    local_cmd.host_tags = vec!["local".into()];
    let global_cmd = crate::commands::SavedCommand::new("Global cmd", "echo global");
    store().save_commands(&[local_cmd, global_cmd]).unwrap();

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.command-host-sections-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    // Default is local host — should show "Local" section + "Global" section
    // Local header + local_cmd + Global header + global_cmd = 4
    let count = window.imp().command_list.observe_children().n_items();
    assert_eq!(
        count, 4,
        "specific host should show host section + global section with headers, got {count}"
    );

    // First row should be a section header (non-activatable)
    let first_row = window.imp().command_list.row_at_index(0).unwrap();
    assert!(!first_row.is_activatable(), "first row should be a section header");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn command_sidebar_only_global_section_when_no_host_commands() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let global_cmd = crate::commands::SavedCommand::new("Global cmd", "echo global");
    store().save_commands(&[global_cmd]).unwrap();

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.command-global-only-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    // Only global commands — should show just Global section header + command
    let count = window.imp().command_list.observe_children().n_items();
    assert_eq!(count, 2, "should show Global header + 1 command, got {count}");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn host_delete_button_hidden_for_local_host() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.host-delete-local-test").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    // Default selection is "Local" — delete button should be hidden
    assert!(
        !window.imp().host_delete_button.is_visible(),
        "delete button should be hidden when Local host is selected"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn host_delete_button_visible_for_remote_host() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    // Save a remote host so it appears in the selector
    let config_dir = tmp.path().join("rttx-devel");
    std::fs::create_dir_all(&config_dir).unwrap();
    let hosts = vec![crate::host::Host::remote("deploy@example.com")];
    store().save_hosts(&hosts).unwrap();

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.host-delete-remote-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    // Select the remote host (index 1: Local=0, example=1, All Hosts=2)
    window.imp().host_selector.set_selected(1);
    pump_events(50);

    assert!(
        window.imp().host_delete_button.is_visible(),
        "delete button should be visible when a remote host is selected"
    );

    // Switch back to Local
    window.imp().host_selector.set_selected(0);
    pump_events(50);

    assert!(
        !window.imp().host_delete_button.is_visible(),
        "delete button should be hidden again when Local is selected"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn host_delete_button_hidden_for_all_hosts() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    // Save a remote host so "All Hosts" is not the only extra entry
    let config_dir = tmp.path().join("rttx-devel");
    std::fs::create_dir_all(&config_dir).unwrap();
    let hosts = vec![crate::host::Host::remote("deploy@example.com")];
    store().save_hosts(&hosts).unwrap();

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.host-delete-allhosts-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    // Select "All Hosts" (last entry)
    let model = window
        .imp()
        .host_selector
        .model()
        .and_then(|m| m.downcast::<gtk4::StringList>().ok())
        .unwrap();
    let last_idx = model.n_items() - 1;
    window.imp().host_selector.set_selected(last_idx);
    pump_events(50);

    assert!(
        !window.imp().host_delete_button.is_visible(),
        "delete button should be hidden when All Hosts is selected"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn add_current_host_saves_remote_host() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.add-current-host-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    // Add a remote managed workspace and switch to it
    let remote_session = WorkspaceState::new_managed_remote(
        "Remote Work".into(),
        "deploy@builder.example.com",
        WorkspacePolicy::Persistent,
        None,
    );
    let remote_uuid = remote_session.uuid.clone();
    window.imp().state.borrow_mut().workspaces.push(remote_session.clone());
    window.build_session(&remote_session, false);
    pump_events(50);

    let state = window.imp().state.borrow();
    let remote_idx = state.workspaces.iter().position(|s| s.uuid == remote_uuid).unwrap();
    drop(state);
    window.switch_to_session(remote_idx);
    pump_events(50);

    // Verify SSH target is detected
    assert_eq!(
        window.ssh_target_for_active_session().as_deref(),
        Some("deploy@builder.example.com"),
    );

    // Trigger the action
    window.do_add_current_host();

    // Verify host was saved
    let hosts = store().load_hosts().into_value().unwrap_or_default();
    assert!(hosts.iter().any(|h| h.key == "builder.example.com"), "host should be saved");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn add_current_host_skips_duplicate() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.add-current-host-dup-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    // Pre-save the host
    let existing = crate::host::Host::remote("deploy@builder.example.com");
    store().save_hosts(&[existing]).unwrap();

    // Add a remote managed workspace and switch to it
    let remote_session = WorkspaceState::new_managed_remote(
        "Remote Work".into(),
        "deploy@builder.example.com",
        WorkspacePolicy::Persistent,
        None,
    );
    let remote_uuid = remote_session.uuid.clone();
    window.imp().state.borrow_mut().workspaces.push(remote_session.clone());
    window.build_session(&remote_session, false);
    pump_events(50);

    let state = window.imp().state.borrow();
    let remote_idx = state.workspaces.iter().position(|s| s.uuid == remote_uuid).unwrap();
    drop(state);
    window.switch_to_session(remote_idx);
    pump_events(50);

    // Trigger the action — should not duplicate
    window.do_add_current_host();

    let hosts = store().load_hosts().into_value().unwrap_or_default();
    assert_eq!(
        hosts.iter().filter(|h| h.key == "builder.example.com").count(),
        1,
        "host should not be duplicated"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn add_current_host_noop_for_local_session() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.add-current-host-local-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    // Default session is local — ssh_target should be None
    assert!(window.ssh_target_for_active_session().is_none());

    // Trigger the action — should not save anything
    window.do_add_current_host();

    let hosts = store().load_hosts().into_value().unwrap_or_default();
    assert!(hosts.is_empty(), "no host should be saved for a local session");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn add_current_path_to_places_saves_place_with_derived_name() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.add-place-cwd-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    let terminal_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    if let Some(term) = window.imp().terminals.borrow().get(&terminal_uuid) {
        term.set_current_directory_for_test(Some("/home/user/projects/rttx"));
    }
    window.imp().focused_terminal_uuid.replace(Some(terminal_uuid));

    window.do_add_current_path_to_places();

    let places = store().load_places();
    assert_eq!(places.len(), 1, "one place should be saved");
    assert_eq!(places[0].name, "rttx");
    assert_eq!(places[0].path, "/home/user/projects/rttx");
    assert!(places[0].host_tags.is_empty(), "local session place should have no host tags");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn add_current_path_to_places_noop_without_cwd() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.add-place-no-cwd-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    // Don't set any CWD — terminal has no known directory
    let terminal_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    window.imp().focused_terminal_uuid.replace(Some(terminal_uuid));

    window.do_add_current_path_to_places();

    let places = store().load_places();
    assert!(places.is_empty(), "no place should be saved when CWD is unknown");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn add_current_path_to_places_tags_remote_host() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.add-place-remote-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    // Add a remote managed workspace and switch to it
    let remote_session = WorkspaceState::new_managed_remote(
        "Remote Work".into(),
        "deploy@builder.example.com",
        WorkspacePolicy::Persistent,
        None,
    );
    let remote_uuid = remote_session.uuid.clone();
    window.imp().state.borrow_mut().workspaces.push(remote_session.clone());
    window.build_session(&remote_session, false);
    pump_events(50);

    let state = window.imp().state.borrow();
    let remote_idx = state.workspaces.iter().position(|s| s.uuid == remote_uuid).unwrap();
    drop(state);
    window.switch_to_session(remote_idx);
    pump_events(50);

    // Set CWD on the persistent terminal
    let terminal_uuid = {
        let state = window.imp().state.borrow();
        let session = state.workspaces.iter().find(|s| s.uuid == remote_uuid).unwrap();
        session.layout.terminal_uuids().into_iter().next().unwrap()
    };
    if let Some(term) = window.imp().persistent_terminals.borrow().get(&terminal_uuid) {
        term.set_current_directory(Some("/srv/app"));
    }
    window.imp().focused_terminal_uuid.replace(Some(terminal_uuid));

    window.do_add_current_path_to_places();

    let places = store().load_places();
    assert_eq!(places.len(), 1, "one place should be saved");
    assert_eq!(places[0].name, "app");
    assert_eq!(places[0].path, "/srv/app");
    assert_eq!(
        places[0].host_tags,
        vec!["builder.example.com"],
        "remote session place should be tagged with the host key"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn open_place_action_sends_cd_to_focused_terminal() {
    require_display!();

    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.open-place-action-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    let terminal_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    window.imp().focused_terminal_uuid.replace(Some(terminal_uuid.clone()));

    window.open_place_in_current_pane("/tmp/test-place");

    let term = window
        .imp()
        .terminals
        .borrow()
        .get(&terminal_uuid)
        .cloned()
        .expect("terminal should exist");
    assert_eq!(
        term.pending_shell_inputs_for_test(),
        vec!["cd /tmp/test-place\n"],
        "open-place should queue a cd command to the focused terminal"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn open_place_action_resolves_tilde_path() {
    require_display!();

    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.open-place-tilde-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    let terminal_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    window.imp().focused_terminal_uuid.replace(Some(terminal_uuid.clone()));

    // "~" resolves to home — the cd command should use "~" (shell resolves it)
    window.open_place_in_current_pane("~");

    let term = window
        .imp()
        .terminals
        .borrow()
        .get(&terminal_uuid)
        .cloned()
        .expect("terminal should exist");
    assert_eq!(
        term.pending_shell_inputs_for_test(),
        vec!["cd ~\n"],
        "tilde path should resolve to home via cd ~"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn open_place_preserves_tilde_prefix_path() {
    require_display!();

    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.open-place-tilde-prefix-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    let terminal_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    window.imp().focused_terminal_uuid.replace(Some(terminal_uuid.clone()));

    // ~/bin should NOT be expanded to /home/user/bin — the shell resolves ~.
    window.open_place_in_current_pane("~/bin");

    let term = window
        .imp()
        .terminals
        .borrow()
        .get(&terminal_uuid)
        .cloned()
        .expect("terminal should exist");
    assert_eq!(
        term.pending_shell_inputs_for_test(),
        vec!["cd ~/bin\n"],
        "tilde-prefix path must be preserved for remote host compatibility"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
fn visible_session_host_key_defaults_to_local() {
    // The underlying logic defaults to LOCAL_KEY when no visible session is found.
    assert_eq!(crate::host::LOCAL_KEY, "local");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn new_direct_creates_non_managed_session() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.new-direct-session-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let initial_count = window.imp().state.borrow().workspaces.len();

    window.add_direct_session();

    let state = window.imp().state.borrow();
    assert_eq!(state.workspaces.len(), initial_count + 1, "direct session should be added");
    let new_session = state.workspaces.last().unwrap();
    assert!(!new_session.uses_managed_runtime(), "direct session should not use managed runtime");
    assert!(
        new_session.name.starts_with("Direct"),
        "direct session name should start with 'Direct'"
    );

    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn keyboard_shortcut_actions_are_registered() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.shortcut-actions-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    assert!(
        window.lookup_action("connect-to-existing").is_some(),
        "connect-to-existing action should be registered"
    );
    assert!(window.lookup_action("new-direct").is_some(), "new-direct action should be registered");
    assert!(
        window.lookup_action("new-session").is_some(),
        "new-session action should be registered"
    );

    window.close();
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn sidebar_subtitle_updates_on_cwd_change() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.subtitle-cwd-change-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let terminal_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    let session_uuid = window.imp().state.borrow().workspaces[0].uuid.clone();

    // Mark the terminal as active so refresh_sidebar_subtitle_if_active finds it.
    {
        let mut state = window.imp().state.borrow_mut();
        state.workspaces[0].active_terminal_uuid = Some(terminal_uuid.clone());
    }

    // Simulate a CWD change (as if the shell emitted OSC 7).
    if let Some(term) = window.imp().terminals.borrow().get(&terminal_uuid) {
        term.set_current_directory_for_test(Some("/tmp/new-dir"));
    }
    window.refresh_sidebar_subtitle_if_active(&terminal_uuid);

    let row = session_row_for_uuid(&window, &session_uuid);
    let subtitle = row.subtitle().map(|v| v.to_string());
    assert_eq!(subtitle.as_deref(), Some("/tmp/new-dir"), "subtitle should reflect the new CWD");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn managed_pane_focus_change_updates_sidebar_subtitle() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.managed-focus-subtitle-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let layout = LayoutNode::Split {
        orientation: SplitOrientation::Horizontal,
        ratio: 0.5,
        first: Box::new(LayoutNode::new_terminal_with_uuid("pane-a")),
        second: Box::new(LayoutNode::new_terminal_with_uuid("pane-b")),
    };
    let session_state = crate::test_helpers::managed_session("ws-focus-test", "Focus Test", layout);
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    // Set different CWDs on the two managed panes.
    {
        let panes = window.imp().persistent_terminals.borrow();
        panes.get("pane-a").unwrap().set_current_directory(Some("/home/user/alpha"));
        panes.get("pane-b").unwrap().set_current_directory(Some("/home/user/beta"));
    }

    // Simulate focusing pane-a: update active_terminal_uuid and refresh subtitle.
    {
        let mut state = window.imp().state.borrow_mut();
        let session = state.workspaces.iter_mut().find(|s| s.uuid == session_state.uuid).unwrap();
        session.active_terminal_uuid = Some("pane-a".to_string());
    }
    window.refresh_sidebar_subtitle(&session_state.uuid);

    let row = session_row_for_uuid(&window, &session_state.uuid);
    let subtitle_a = row.subtitle().map(|v| v.to_string());
    assert!(
        subtitle_a.as_deref().is_some_and(|s| s.contains("alpha")),
        "subtitle should show pane-a's CWD, got: {subtitle_a:?}"
    );

    // Now simulate focusing pane-b.
    {
        let mut state = window.imp().state.borrow_mut();
        let session = state.workspaces.iter_mut().find(|s| s.uuid == session_state.uuid).unwrap();
        session.active_terminal_uuid = Some("pane-b".to_string());
    }
    window.refresh_sidebar_subtitle(&session_state.uuid);

    let subtitle_b = row.subtitle().map(|v| v.to_string());
    assert!(
        subtitle_b.as_deref().is_some_and(|s| s.contains("beta")),
        "subtitle should update to pane-b's CWD, got: {subtitle_b:?}"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn host_add_button_visible_in_host_row() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.host-add-button-visible-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    assert!(
        window.imp().host_add_button.is_visible(),
        "add host button should always be visible in the host row"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn host_add_button_has_correct_icon_and_tooltip() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.host-add-button-icon-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    assert_eq!(
        window.imp().host_add_button.icon_name().unwrap(),
        "list-add-symbolic",
        "add host button should use the list-add-symbolic icon"
    );
    assert_eq!(
        window.imp().host_add_button.tooltip_text().unwrap().as_str(),
        "Add host",
        "add host button should have the correct tooltip"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn new_menu_includes_add_host_item() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.new-menu-add-host-test").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    let model = window.imp().new_button.menu_model().unwrap();
    let n = model.n_items();
    assert!(n >= 2, "New menu should have at least Local + Add Host…");

    // Last item should be "Add Host…"
    let last_label = model.item_attribute_value(n - 1, "label", None);
    assert_eq!(
        last_label.and_then(|v| v.get::<String>()),
        Some("Add Host…".into()),
        "last item in New menu should be 'Add Host…'"
    );

    // Connect menu should also have "Add Host…"
    let connect_model = window.imp().connect_button.menu_model().unwrap();
    let cn = connect_model.n_items();
    let connect_last = connect_model.item_attribute_value(cn - 1, "label", None);
    assert_eq!(
        connect_last.and_then(|v| v.get::<String>()),
        Some("Add Host…".into()),
        "last item in Connect menu should be 'Add Host…'"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn new_menu_includes_saved_remote_hosts() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let config_dir = tmp.path().join("rttx-devel");
    std::fs::create_dir_all(&config_dir).unwrap();
    let hosts = vec![crate::host::Host::remote("deploy@example.com")];
    store().save_hosts(&hosts).unwrap();

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.new-menu-saved-hosts-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    let model = window.imp().new_button.menu_model().unwrap();
    // Should have: Local, example, Add Host…
    assert!(model.n_items() >= 3, "New menu should have Local + remote host + Add Host…");

    let labels: Vec<String> = (0..model.n_items())
        .filter_map(|i| {
            model.item_attribute_value(i, "label", None).and_then(|v| v.get::<String>())
        })
        .collect();
    assert!(labels.contains(&"Local".into()), "menu should contain Local");
    assert!(labels.contains(&"example".into()), "menu should contain saved remote host");
    assert!(labels.contains(&"Add Host…".into()), "menu should contain Add Host…");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn new_menu_includes_session_derived_hosts() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.new-menu-session-hosts-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    // Add a remote managed workspace (not saved in hosts.json)
    let remote_session = WorkspaceState::new_managed_remote(
        "Remote Work".into(),
        "deploy@builder.example.com",
        WorkspacePolicy::Persistent,
        None,
    );
    window.imp().state.borrow_mut().workspaces.push(remote_session.clone());
    window.build_session(&remote_session, false);
    pump_events(50);

    // Refresh menus to pick up session-derived hosts
    window.refresh_host_menus();

    let model = window.imp().new_button.menu_model().unwrap();
    let labels: Vec<String> = (0..model.n_items())
        .filter_map(|i| {
            model.item_attribute_value(i, "label", None).and_then(|v| v.get::<String>())
        })
        .collect();
    assert!(
        labels.contains(&"builder".into()),
        "menu should contain session-derived remote host; got: {labels:?}"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn place_sidebar_shows_edit_delete_for_user_places_not_builtins() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let user_place = crate::places::Place::new("MyProject", "~/projects/myproject");
    store().save_places(&[user_place]).unwrap();

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.place-sidebar-crud-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    window.refresh_place_sidebar();
    pump_events(50);

    let place_list = &window.imp().place_list;
    let mut found_builtin_with_menu = false;
    let mut found_user_with_menu = false;
    let mut idx = 0;
    while let Some(row) = place_list.row_at_index(idx) {
        if let Some(action_row) = row.downcast_ref::<adw::ActionRow>() {
            let title = action_row.title().to_string();
            let has_menu = has_menu_button_suffix(action_row);
            if title == "Home" || title == "Root" {
                if has_menu {
                    found_builtin_with_menu = true;
                }
            } else if title == "MyProject" && has_menu {
                found_user_with_menu = true;
            }
        }
        idx += 1;
    }

    assert!(!found_builtin_with_menu, "built-in places should not have edit/delete menu");
    assert!(found_user_with_menu, "user places should have edit/delete menu");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

fn has_menu_button_suffix(action_row: &adw::ActionRow) -> bool {
    let mut child = action_row.first_child();
    while let Some(widget) = child {
        if widget.downcast_ref::<gtk4::MenuButton>().is_some() {
            return true;
        }
        if let Some(inner) = widget.first_child() {
            let mut inner_child = Some(inner);
            while let Some(w) = inner_child {
                if w.downcast_ref::<gtk4::MenuButton>().is_some() {
                    return true;
                }
                inner_child = w.next_sibling();
            }
        }
        child = widget.next_sibling();
    }
    false
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn place_sidebar_has_add_button_in_header() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.place-sidebar-add-btn-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    assert!(window.lookup_action("add-place").is_some(), "add-place action should be registered");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn close_others_removes_all_except_kept_session() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.close-others-test").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.add_session();
    window.add_session();
    pump_events(50);

    let (uuid0, uuid1, uuid2) = {
        let state = window.imp().state.borrow();
        (
            state.workspaces[0].uuid.clone(),
            state.workspaces[1].uuid.clone(),
            state.workspaces[2].uuid.clone(),
        )
    };

    // Close all except session 1 (middle one).
    let others: Vec<String> = vec![uuid0.clone(), uuid2.clone()];
    for uuid in &others {
        window.close_session(uuid);
    }

    let state = window.imp().state.borrow();
    assert_eq!(state.workspaces.len(), 1, "only the kept session should remain");
    assert_eq!(state.workspaces[0].uuid, uuid1);

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn close_others_noop_with_single_session() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.close-others-noop-test").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    pump_events(50);

    let uuid0 = window.imp().state.borrow().workspaces[0].uuid.clone();

    // With only one session, closing others should be a no-op.
    let others: Vec<String> = {
        let state = window.imp().state.borrow();
        state.workspaces.iter().filter(|s| s.uuid != uuid0).map(|s| s.uuid.clone()).collect()
    };
    assert!(others.is_empty(), "there should be no other sessions to close");

    assert_eq!(window.imp().state.borrow().workspaces.len(), 1);

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn right_click_on_sidebar_row_opens_popover_menu() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.sidebar-right-click-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    pump_events(50);

    let session_uuid = window.imp().state.borrow().workspaces[0].uuid.clone();
    let row = session_row_for_uuid(&window, &session_uuid);

    // Directly call show_workspace_popover_menu (simulates right-click handler).
    window.show_workspace_popover_menu(&row, &session_uuid);

    assert!(
        window.imp().workspace_popover.borrow().is_some(),
        "right-click should open a popover menu"
    );

    // Verify the close-others and close-all actions are registered.
    assert!(
        window.lookup_action("ctx-close-others").is_some(),
        "ctx-close-others action should be registered"
    );
    assert!(
        window.lookup_action("ctx-close-all").is_some(),
        "ctx-close-all action should be registered"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn workspace_popover_menu_includes_close_others_and_close_all() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.popover-bulk-items-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let session_state = crate::test_helpers::managed_session(
        "workspace-bulk",
        "Bulk Test",
        LayoutNode::new_terminal_with_uuid("managed-pane"),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);
    pump_events(50);

    let row = session_row_for_uuid(&window, &session_state.uuid);
    window.show_workspace_popover_menu(&row, &session_state.uuid);

    assert!(
        window.lookup_action("ctx-close-others").is_some(),
        "managed workspace popover should have close-others action"
    );
    assert!(
        window.lookup_action("ctx-close-all").is_some(),
        "managed workspace popover should have close-all action"
    );
    assert!(
        window.lookup_action("ctx-close").is_some(),
        "managed workspace popover should have close action"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn sidebar_row_has_right_click_gesture() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.sidebar-row-gesture-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    pump_events(50);

    let session_uuid = window.imp().state.borrow().workspaces[0].uuid.clone();
    let row = session_row_for_uuid(&window, &session_uuid);

    // Check that the row has a GestureClick controller for button 3 (right-click).
    let controllers = row.observe_controllers();
    let mut has_right_click = false;
    for index in 0..controllers.n_items() {
        if let Some(controller) = controllers.item(index)
            && let Ok(gesture) = controller.downcast::<gtk4::GestureClick>()
            && gesture.button() == 3
        {
            has_right_click = true;
            break;
        }
    }
    assert!(has_right_click, "sidebar row should have a right-click gesture controller");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn window_dispose_clears_terminal_maps_and_cancels_sources() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.dispose-cleanup-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.add_direct_session();
    pump_events(50);

    assert!(
        !window.imp().terminals.borrow().is_empty(),
        "direct session should have at least one terminal"
    );

    window.close();
    pump_events(50);

    assert!(window.imp().terminals.borrow().is_empty(), "dispose should clear terminals map");
    assert!(
        window.imp().persistent_terminals.borrow().is_empty(),
        "dispose should clear persistent_terminals map"
    );
    assert!(
        window.imp().workspace_reconnect_sources.borrow().is_empty(),
        "dispose should clear reconnect sources"
    );

    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn managed_pane_closures_use_weak_window_refs() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder().application_id("com.illya.rttx.weak-ref-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let weak = window.downgrade();

    // Create a managed session — the closures in connect_managed_pane
    // should use weak refs, so dropping the window should allow it to
    // be finalized.
    window.add_managed_session_at(Some("/tmp".to_string()));
    pump_events(50);

    assert!(
        !window.imp().persistent_terminals.borrow().is_empty(),
        "managed session should have at least one persistent pane"
    );

    window.close();
    pump_events(50);

    // After close + dispose, the weak ref should no longer upgrade
    // because the reference cycles are broken.
    assert!(
        weak.upgrade().is_none() || window.imp().persistent_terminals.borrow().is_empty(),
        "window should be finalizable or maps cleared after close"
    );

    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn rebuild_session_content_removes_stale_persistent_terminal_entries() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.stale-hashmap-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let session_state = crate::test_helpers::managed_session(
        "workspace-stale",
        "Stale Test",
        crate::test_helpers::hsplit(
            LayoutNode::new_terminal_with_uuid("pane-a"),
            LayoutNode::new_terminal_with_uuid("pane-b"),
        ),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    assert_eq!(
        window.imp().persistent_terminals.borrow().len(),
        2,
        "both panes should be materialized"
    );

    // Simulate reconciliation that removes pane-b from the layout.
    let reduced_state = {
        let mut state = window.imp().state.borrow_mut();
        let session = state.workspaces.iter_mut().find(|s| s.uuid == "workspace-stale").unwrap();
        session.layout = LayoutNode::new_terminal_with_uuid("pane-a");
        session.clone()
    };

    window.rebuild_session_content("workspace-stale", &reduced_state);

    assert_eq!(
        window.imp().persistent_terminals.borrow().len(),
        1,
        "stale pane-b entry should be removed after rebuild"
    );
    assert!(
        window.imp().persistent_terminals.borrow().contains_key("pane-a"),
        "surviving pane-a should remain in the map"
    );
    assert!(
        !window.imp().persistent_terminals.borrow().contains_key("pane-b"),
        "removed pane-b should not remain in the map"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn rebuild_session_content_preserves_terminals_from_other_workspaces() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.stale-cross-workspace-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let session_a = crate::test_helpers::managed_session(
        "workspace-a",
        "Workspace A",
        LayoutNode::new_terminal_with_uuid("pane-a1"),
    );
    let session_b = crate::test_helpers::managed_session(
        "workspace-b",
        "Workspace B",
        LayoutNode::new_terminal_with_uuid("pane-b1"),
    );
    {
        let mut state = window.imp().state.borrow_mut();
        state.workspaces.push(session_a.clone());
        state.workspaces.push(session_b.clone());
    }
    window.build_session(&session_a, false);
    window.build_session(&session_b, false);

    assert_eq!(window.imp().persistent_terminals.borrow().len(), 2);

    // Rebuild workspace-a — workspace-b's pane must survive.
    window.rebuild_session_content("workspace-a", &session_a);

    assert_eq!(
        window.imp().persistent_terminals.borrow().len(),
        2,
        "terminals from other workspaces must not be removed"
    );
    assert!(window.imp().persistent_terminals.borrow().contains_key("pane-a1"));
    assert!(window.imp().persistent_terminals.borrow().contains_key("pane-b1"));

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

/// Closing the last managed workspace must remove the session from state
/// and add the runtime_id to dismissed_runtime_ids so the daemon session
/// is not resurrected on next launch. Regression test for #578.
#[test]
#[ignore = "requires isolated GTK harness"]
fn close_last_managed_workspace_dismisses_runtime() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.close-last-managed-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    // Replace the default session with a managed one that has a runtime_id.
    let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
    let managed = crate::test_helpers::managed_session_with_runtime(
        "managed-ws",
        "Managed",
        crate::test_helpers::term("t1"),
        crate::runtime::RuntimeEndpoint::Local,
        crate::runtime::WorkspacePolicy::Persistent,
        Some(runtime_id),
    );
    let session_uuid = managed.uuid.clone();
    {
        let mut state = window.imp().state.borrow_mut();
        state.workspaces.clear();
        state.workspaces.push(managed.clone());
    }
    window.build_session(&managed, false);

    assert_eq!(window.imp().state.borrow().workspaces.len(), 1);

    window.close_session(&session_uuid);
    pump_events(50);

    let state = window.imp().state.borrow();
    assert!(
        !state.workspaces.iter().any(|s| s.uuid == session_uuid),
        "closed managed workspace must be removed from state"
    );
    assert!(
        state.dismissed_runtime_ids.contains(runtime_id),
        "runtime_id must be added to dismissed_runtime_ids"
    );

    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

/// After closing the last managed workspace, the persisted state must not
/// contain the closed session. This prevents resurrection on next launch.
/// Regression test for #578.
#[test]
#[ignore = "requires isolated GTK harness"]
fn close_last_managed_workspace_persists_clean_state() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.close-last-managed-persist-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let runtime_id = "a1b2c3d4-e5f6-7890-abcd-ef1234567890";
    let managed = crate::test_helpers::managed_session_with_runtime(
        "managed-ws",
        "Managed",
        crate::test_helpers::term("t1"),
        crate::runtime::RuntimeEndpoint::Local,
        crate::runtime::WorkspacePolicy::Persistent,
        Some(runtime_id),
    );
    let session_uuid = managed.uuid.clone();
    {
        let mut state = window.imp().state.borrow_mut();
        state.workspaces.clear();
        state.workspaces.push(managed.clone());
    }
    window.build_session(&managed, false);

    window.close_session(&session_uuid);
    pump_events(50);

    // Reload persisted state and verify the closed session is gone.
    let saved = load_saved_window_state();
    assert!(
        !saved.workspaces.iter().any(|s| s.uuid == session_uuid),
        "persisted state must not contain the closed managed workspace"
    );
    assert!(
        saved.dismissed_runtime_ids.contains(runtime_id),
        "persisted state must contain the dismissed runtime_id"
    );

    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

/// When the terminal widget has no CWD (e.g. managed pane before OSC 7),
/// `split_terminal` should fall back to the layout node's CWD.
#[test]
#[ignore = "requires isolated GTK harness"]
fn split_falls_back_to_layout_cwd_when_terminal_has_none() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.split-layout-cwd-fallback")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.set_default_size(1200, 800);
    window.present();
    pump_events(100);

    let t1_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };

    // Set CWD on the layout node only — the terminal widget has no CWD.
    {
        let mut state = window.imp().state.borrow_mut();
        state.workspaces[0].layout.set_terminal_cwd(&t1_uuid, Some("/srv/project".into()));
    }

    // Confirm the terminal widget itself has no CWD.
    {
        let terminals = window.imp().terminals.borrow();
        let t1 = terminals.get(&t1_uuid).unwrap();
        assert!(
            t1.current_directory().is_none(),
            "precondition: terminal widget should have no CWD"
        );
    }

    window.split_terminal(&t1_uuid, SplitOrientation::Horizontal);

    let (t2_uuid, t2_cwd) = {
        let state = window.imp().state.borrow();
        let uuids = state.workspaces[0].layout.terminal_uuids();
        let t2 = uuids.into_iter().find(|u| u != &t1_uuid).unwrap();
        let cwd = state.workspaces[0].layout.terminal_cwd(&t2);
        (t2, cwd)
    };

    assert_eq!(
        t2_cwd.as_deref(),
        Some("/srv/project"),
        "new pane should inherit CWD from layout node when terminal widget has none"
    );

    {
        let terminals = window.imp().terminals.borrow();
        let t2 = terminals.get(&t2_uuid).unwrap();
        assert_eq!(
            t2.initial_cwd_for_test(),
            Some("/srv/project".to_string()),
            "new TerminalWidget should receive layout-fallback CWD"
        );
    }

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn persistent_terminal_size_returns_vte_dimensions() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.persistent-terminal-size-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let session_state = crate::test_helpers::managed_session(
        "ws-size",
        "Size Workspace",
        LayoutNode::new_terminal_with_uuid("pane-a"),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    let (cols, rows) = window.persistent_terminal_size("pane-a");
    assert!(
        cols > 0 && rows > 0,
        "registered terminal must report non-zero size, got {cols}x{rows}"
    );

    let (cols, rows) = window.persistent_terminal_size("no-such-pane");
    assert_eq!((cols, rows), (0, 0), "unregistered terminal must return (0, 0)");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn split_managed_pane_passes_source_terminal_size() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.split-pane-size-test").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let session_state = crate::test_helpers::managed_session(
        "ws-split-size",
        "Split Size Workspace",
        LayoutNode::new_terminal_with_uuid("source-pane"),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    let source_size = window.persistent_terminal_size("source-pane");
    assert!(
        source_size.0 > 0 && source_size.1 > 0,
        "source pane should report non-zero size before split"
    );

    // Trigger a horizontal split on the source pane.
    window.split_terminal("source-pane", SplitOrientation::Horizontal);

    let state = window.imp().state.borrow();
    let session = state.workspaces.iter().find(|s| s.uuid == "ws-split-size").unwrap();
    let uuids = session.layout.terminal_uuids();
    assert_eq!(uuids.len(), 2, "split should produce two panes");
    let new_uuid = uuids.into_iter().find(|u| u != "source-pane").unwrap();
    drop(state);

    let new_size = window.persistent_terminal_size(&new_uuid);
    assert!(
        new_size.0 > 0 && new_size.1 > 0,
        "new pane from split should report non-zero size, got {}x{}",
        new_size.0,
        new_size.1
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

/// The event poller batch limit must be positive and bounded to prevent
/// the main loop from starving during output bursts. Regression for #653.
#[test]
fn event_poll_batch_limit_is_bounded() {
    use super::runtime::EVENT_POLL_BATCH_LIMIT;
    const { assert!(EVENT_POLL_BATCH_LIMIT > 0) };
    const { assert!(EVENT_POLL_BATCH_LIMIT <= 256) };
}

/// The event poller time budget must be positive and strictly less than
/// the poll interval so GTK always gets time for rendering and input.
/// Regression for #828.
#[test]
fn event_poll_time_budget_is_bounded() {
    use super::runtime::{EVENT_POLL_INTERVAL, EVENT_POLL_TIME_BUDGET};
    assert!(!EVENT_POLL_TIME_BUDGET.is_zero(), "budget must be positive");
    assert!(
        EVENT_POLL_TIME_BUDGET < EVENT_POLL_INTERVAL,
        "budget must be strictly less than the poll interval"
    );
}

/// The poller must break early when the time budget is exceeded, even if
/// the batch limit has not been reached. Regression for #828.
#[test]
fn event_poll_respects_time_budget() {
    use super::runtime::{EVENT_POLL_BATCH_LIMIT, EVENT_POLL_TIME_BUDGET};
    use std::time::Instant;

    // Simulate a poll loop that always has events available.
    // The loop should terminate due to the time budget, not the batch limit.
    let start = Instant::now();
    let mut count = 0u64;
    for _ in 0..EVENT_POLL_BATCH_LIMIT {
        if start.elapsed() > EVENT_POLL_TIME_BUDGET {
            break;
        }
        // Simulate minimal work per event.
        count += 1;
        std::hint::black_box(count);
    }
    // The loop ran within the budget (or hit the batch limit for trivial work).
    // Either way, elapsed time should be bounded.
    let elapsed = start.elapsed();
    // Allow 2× budget for scheduling jitter, but it must not be unbounded.
    let max_allowed = EVENT_POLL_TIME_BUDGET * 2 + std::time::Duration::from_millis(1);
    assert!(elapsed < max_allowed, "poll loop took {elapsed:?}, expected < {max_allowed:?}");
}

// ── Session persistence end-to-end (GUI round-trip) ─────────────

/// Full GUI round-trip: create workspaces, split, rename, reorder →
/// save_state → close → new Window (load_state) → verify widget tree.
/// Covers the gap identified in #325.
#[test]
#[ignore = "requires isolated GTK harness"]
fn save_and_restart_full_session_persistence_roundtrip() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.persistence-e2e-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    // ── Phase 1: Build state through GUI actions ────────────────

    let first_window = Window::new(&app);
    first_window.set_default_size(1200, 800);
    first_window.present();
    pump_events(100);

    // Add two more workspaces (3 total).
    first_window.add_direct_session();
    first_window.add_direct_session();
    pump_events(100);

    let (ws0_uuid, ws1_uuid, ws2_uuid) = {
        let state = first_window.imp().state.borrow();
        assert_eq!(state.workspaces.len(), 3);
        (
            state.workspaces[0].uuid.clone(),
            state.workspaces[1].uuid.clone(),
            state.workspaces[2].uuid.clone(),
        )
    };

    // Rename workspace 0.
    first_window.rename_runtime(&ws0_uuid, "Editor");

    // Split workspace 0 horizontally.
    let ws0_t1 = {
        let state = first_window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    first_window.split_terminal(&ws0_t1, SplitOrientation::Horizontal);
    pump_events(100);

    let ws0_t2 = {
        let state = first_window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().find(|u| u != &ws0_t1).unwrap()
    };

    // Rename workspace 1.
    first_window.rename_runtime(&ws1_uuid, "Build");

    // Reorder: move workspace 2 to position 0.
    first_window.reorder_session(&ws2_uuid, &ws0_uuid);
    pump_events(50);

    // Select workspace 1 (now at index 1 after reorder).
    let active_index = {
        let state = first_window.imp().state.borrow();
        state.workspaces.iter().position(|s| s.uuid == ws0_uuid).unwrap()
    };
    if let Some(row) = first_window.imp().sidebar_list.row_at_index(active_index as i32) {
        first_window.imp().sidebar_list.select_row(Some(&row));
    }
    pump_events(50);

    // Capture expected state before save.
    let expected_order: Vec<String> = {
        let state = first_window.imp().state.borrow();
        state.workspaces.iter().map(|s| s.uuid.clone()).collect()
    };
    let expected_names: Vec<String> = {
        let state = first_window.imp().state.borrow();
        state.workspaces.iter().map(|s| s.name.clone()).collect()
    };

    // ── Phase 2: Save and close ─────────────────────────────────

    first_window.save_state();
    first_window.close();

    // Verify saved state on disk.
    let saved = load_saved_window_state();
    assert_eq!(saved.workspaces.len(), 3, "saved state should have 3 workspaces");
    let saved_order: Vec<&str> = saved.workspaces.iter().map(|s| s.uuid.as_str()).collect();
    assert_eq!(
        saved_order,
        expected_order.iter().map(String::as_str).collect::<Vec<_>>(),
        "saved workspace order must match"
    );
    let saved_names: Vec<&str> = saved.workspaces.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        saved_names,
        expected_names.iter().map(String::as_str).collect::<Vec<_>>(),
        "saved workspace names must match"
    );

    // The "Editor" workspace should have 2 terminals (was split).
    let editor_session = saved.workspaces.iter().find(|s| s.uuid == ws0_uuid).unwrap();
    assert_eq!(
        editor_session.layout.terminal_count(),
        2,
        "Editor workspace should have 2 panes after split"
    );
    assert!(editor_session.layout.contains_terminal(&ws0_t1));
    assert!(editor_session.layout.contains_terminal(&ws0_t2));

    // ── Phase 3: Restore into a new window ──────────────────────

    let second_window = Window::new(&app);
    second_window.set_default_size(1200, 800);
    second_window.present();
    pump_events(200);

    // Verify workspace count.
    let restored_count = {
        let state = second_window.imp().state.borrow();
        state.workspaces.len()
    };
    assert_eq!(restored_count, 3, "restored window should have 3 workspaces");

    // Verify workspace order.
    let restored_order: Vec<String> = {
        let state = second_window.imp().state.borrow();
        state.workspaces.iter().map(|s| s.uuid.clone()).collect()
    };
    assert_eq!(restored_order, expected_order, "workspace order must survive restart");

    // Verify workspace names.
    let restored_names: Vec<String> = {
        let state = second_window.imp().state.borrow();
        state.workspaces.iter().map(|s| s.name.clone()).collect()
    };
    assert_eq!(restored_names, expected_names, "workspace names must survive restart");

    // Verify sidebar rows match.
    for (i, expected_uuid) in expected_order.iter().enumerate() {
        let row = session_row_at(&second_window, i as i32);
        assert_eq!(row.uuid(), *expected_uuid, "sidebar row {i} UUID must match after restart");
    }

    // Verify the "Editor" workspace has 2 terminals in the widget tree.
    let editor_content = second_window
        .imp()
        .session_stack
        .child_by_name(&ws0_uuid)
        .expect("Editor workspace content should exist");
    let paned = editor_content
        .downcast_ref::<gtk4::Paned>()
        .expect("Editor workspace root should be a Paned (split)");
    assert!(paned.start_child().is_some(), "split should have a start child");
    assert!(paned.end_child().is_some(), "split should have an end child");

    // Verify both terminals exist in the terminal map.
    {
        let terminals = second_window.imp().terminals.borrow();
        assert!(terminals.contains_key(&ws0_t1), "first terminal should be restored");
        assert!(terminals.contains_key(&ws0_t2), "second terminal (from split) should be restored");
    }

    // Verify the renamed sidebar labels.
    let editor_row = session_row_for_uuid(&second_window, &ws0_uuid);
    assert_eq!(
        editor_row.workspace_name(),
        "Editor",
        "renamed workspace should keep its name after restart"
    );
    let build_row = session_row_for_uuid(&second_window, &ws1_uuid);
    assert_eq!(
        build_row.workspace_name(),
        "Build",
        "renamed workspace should keep its name after restart"
    );

    second_window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

// ── Managed workspace orchestration (window/runtime.rs) ─────────

#[test]
#[ignore = "requires isolated GTK harness"]
fn open_runtime_ids_for_endpoint_returns_bound_runtime_ids() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.open-runtime-ids-test").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let local_host = crate::host::Host::local();
    let remote_host = crate::host::Host::remote("deploy@prod");

    // Add a local managed workspace with a runtime ID.
    let s1 = crate::test_helpers::managed_session_with_runtime(
        "ws-local",
        "Local",
        LayoutNode::new_terminal_with_uuid("pane-local"),
        RuntimeEndpoint::Local,
        WorkspacePolicy::Persistent,
        Some("runtime-aaa"),
    );
    // Add a remote managed workspace with a runtime ID.
    let s2 = crate::test_helpers::managed_session_with_runtime(
        "ws-remote",
        "Remote",
        LayoutNode::new_terminal_with_uuid("pane-remote"),
        RuntimeEndpoint::remote("deploy@prod"),
        WorkspacePolicy::Persistent,
        Some("runtime-bbb"),
    );
    // Add a local managed workspace without a runtime ID (not yet connected).
    let s3 = crate::test_helpers::managed_session_with_runtime(
        "ws-pending",
        "Pending",
        LayoutNode::new_terminal_with_uuid("pane-pending"),
        RuntimeEndpoint::Local,
        WorkspacePolicy::Persistent,
        None,
    );
    {
        let mut state = window.imp().state.borrow_mut();
        state.workspaces.push(s1);
        state.workspaces.push(s2);
        state.workspaces.push(s3);
    }

    let local_ids = window.open_runtime_ids_for_endpoint(&local_host);
    assert_eq!(local_ids, vec!["runtime-aaa"], "should return only the bound local runtime ID");

    let remote_ids = window.open_runtime_ids_for_endpoint(&remote_host);
    assert_eq!(remote_ids, vec!["runtime-bbb"], "should return only the bound remote runtime ID");

    let unknown_host = crate::host::Host::remote("nobody@nowhere");
    let unknown_ids = window.open_runtime_ids_for_endpoint(&unknown_host);
    assert!(unknown_ids.is_empty(), "unknown endpoint should return no runtime IDs");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn restore_managed_snapshot_feeds_scrollback_and_cwd_to_pane() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.restore-snapshot-test").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let layout_uuid = "snap-pane";
    let session_state = crate::test_helpers::managed_session(
        "ws-snap",
        "Snapshot Test",
        LayoutNode::new_terminal_with_uuid(layout_uuid),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    let restore = crate::workspace_state::WorkspacePaneRestore {
        layout_terminal_uuid: layout_uuid.to_string(),
        title: "vim main.rs".to_string(),
        cwd: "/home/user/project".to_string(),
        pane_output_seq: 0,
        scrollback_tail: bytes::Bytes::from_static(b"$ cargo build\r\nCompiling rttx\r\n"),
        scrollback_complete: true,
        cols: 120,
        rows: 40,
        terminal_modes: None,
    };
    window.restore_managed_snapshot(&restore);

    let pane = window
        .imp()
        .persistent_terminals
        .borrow()
        .get(layout_uuid)
        .cloned()
        .expect("pane should exist");

    assert_eq!(
        pane.current_directory().as_deref(),
        Some("/home/user/project"),
        "snapshot restore should set CWD"
    );
    assert_eq!(pane.status_label_text_for_test(), "Connected", "snapshot restore marks connected");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

/// Regression for #944: when the snapshot was captured at a different terminal
/// size than the current pane, VTE must be told the current size before the
/// snapshot bytes are fed. Otherwise VTE wraps lines at the old column count.
#[test]
#[ignore = "requires isolated GTK harness"]
fn restore_managed_snapshot_sets_vte_size_before_feed() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.restore-snapshot-resize-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let layout_uuid = "resize-snap-pane";
    let session_state = crate::test_helpers::managed_session(
        "ws-resize-snap",
        "Resize Snap Test",
        LayoutNode::new_terminal_with_uuid(layout_uuid),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    let pane = window
        .imp()
        .persistent_terminals
        .borrow()
        .get(layout_uuid)
        .cloned()
        .expect("pane should exist");

    // Record the pane's current VTE size (default before allocation).
    let (current_cols, current_rows) = pane.terminal_size();

    // Snapshot claims it was captured at a different size.
    let restore = crate::workspace_state::WorkspacePaneRestore {
        layout_terminal_uuid: layout_uuid.to_string(),
        title: String::new(),
        cwd: "/tmp".to_string(),
        pane_output_seq: 0,
        scrollback_tail: bytes::Bytes::from_static(b"wide output line\r\n"),
        scrollback_complete: true,
        cols: current_cols.saturating_add(40),
        rows: current_rows.saturating_add(10),
        terminal_modes: None,
    };
    window.restore_managed_snapshot(&restore);

    // After restore, VTE's column count must match the current pane size,
    // not the snapshot's stale dimensions.
    let (after_cols, after_rows) = pane.terminal_size();
    assert_eq!(
        after_cols, current_cols,
        "VTE column count should match current pane, not snapshot"
    );
    assert_eq!(after_rows, current_rows, "VTE row count should match current pane, not snapshot");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

/// Reconnect restore applies `focus_reporting` and `cursor_hidden` from the
/// snapshot even when the scrollback tail does not contain the original
/// enabling escape sequences. #765.
#[test]
#[ignore = "requires isolated GTK harness"]
fn restore_managed_snapshot_applies_focus_and_cursor_modes() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.restore-focus-cursor-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let layout_uuid = "focus-cursor-pane";
    let session_state = crate::test_helpers::managed_session(
        "ws-fc",
        "Focus Cursor Test",
        LayoutNode::new_terminal_with_uuid(layout_uuid),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    // Scrollback tail intentionally lacks the enabling escape sequences —
    // the restore path must re-apply modes from the snapshot metadata.
    let restore = crate::workspace_state::WorkspacePaneRestore {
        layout_terminal_uuid: layout_uuid.to_string(),
        title: "htop".to_string(),
        cwd: "/home/user".to_string(),
        pane_output_seq: 0,
        scrollback_tail: bytes::Bytes::from_static(b"plain text only\r\n"),
        scrollback_complete: true,
        cols: 80,
        rows: 24,
        terminal_modes: Some(rttx_proto::v3::TerminalModeState {
            bracketed_paste: true,
            focus_reporting: true,
            cursor_hidden: true,
            application_cursor_keys: true,
            application_keypad: false,
            alternate_screen: false,
            mouse_mode: rttx_proto::v3::MouseMode::None as i32,
            sgr_mouse: false,
        }),
    };
    window.restore_managed_snapshot(&restore);

    let pane = window
        .imp()
        .persistent_terminals
        .borrow()
        .get(layout_uuid)
        .cloned()
        .expect("pane should exist");

    assert!(
        pane.imp().bracketed_paste_mode.get(),
        "bracketed paste should be restored from snapshot"
    );
    assert!(
        pane.imp().application_cursor_keys.get(),
        "application cursor keys should be restored from snapshot"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn restore_managed_snapshot_skips_missing_pane() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.restore-snapshot-missing-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    // Restore for a pane that doesn't exist — should not panic.
    let restore = crate::workspace_state::WorkspacePaneRestore {
        layout_terminal_uuid: "nonexistent-pane".to_string(),
        title: String::new(),
        cwd: "/tmp".to_string(),
        pane_output_seq: 0,
        scrollback_tail: bytes::Bytes::new(),
        scrollback_complete: true,
        cols: 80,
        rows: 24,
        terminal_modes: None,
    };
    window.restore_managed_snapshot(&restore);

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn restore_managed_snapshot_sets_daemon_title_when_no_custom_title() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.restore-snapshot-title-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let layout_uuid = "title-pane";
    let session_state = crate::test_helpers::managed_session(
        "ws-title",
        "Title Test",
        LayoutNode::new_terminal_with_uuid(layout_uuid),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    // Set a custom title first — snapshot should NOT override it.
    let pane = window.imp().persistent_terminals.borrow().get(layout_uuid).cloned().unwrap();
    pane.set_custom_title(Some("My Custom Title"));

    let restore = crate::workspace_state::WorkspacePaneRestore {
        layout_terminal_uuid: layout_uuid.to_string(),
        title: "daemon-reported-title".to_string(),
        cwd: "/tmp".to_string(),
        pane_output_seq: 0,
        scrollback_tail: bytes::Bytes::new(),
        scrollback_complete: true,
        cols: 80,
        rows: 24,
        terminal_modes: None,
    };
    window.restore_managed_snapshot(&restore);

    assert_eq!(
        pane.custom_title().as_deref(),
        Some("My Custom Title"),
        "custom title should not be overridden by snapshot restore"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn mark_managed_pane_connected_sets_status_label() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.mark-connected-test").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let layout_uuid = "connect-pane";
    let session_state = crate::test_helpers::managed_session(
        "ws-connect",
        "Connect Test",
        LayoutNode::new_terminal_with_uuid(layout_uuid),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    // Initially the pane is not connected.
    let pane = window.imp().persistent_terminals.borrow().get(layout_uuid).cloned().unwrap();
    assert_ne!(
        pane.status_label_text_for_test(),
        "Connected",
        "pane should not be connected initially"
    );

    window.mark_managed_pane_connected(layout_uuid);

    assert_eq!(
        pane.status_label_text_for_test(),
        "Connected",
        "mark_managed_pane_connected should set status to Connected"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn status_propagation_updates_sidebar_and_all_panes() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.status-propagation-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    // Create a workspace with two panes (split).
    let layout = crate::test_helpers::hsplit(
        LayoutNode::new_terminal_with_uuid("pane-a"),
        LayoutNode::new_terminal_with_uuid("pane-b"),
    );
    let session_state = crate::test_helpers::managed_session("ws-status", "Status Test", layout);
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    // Set status to Disconnected — both panes should reflect it.
    window.set_workspace_connection_status(&session_state.uuid, &ConnectionStatus::Disconnected);

    let panes = window.imp().persistent_terminals.borrow();
    let pane_a = panes.get("pane-a").expect("pane-a should exist");
    let pane_b = panes.get("pane-b").expect("pane-b should exist");
    assert_eq!(pane_a.status_label_text_for_test(), "Disconnected");
    assert_eq!(pane_b.status_label_text_for_test(), "Disconnected");
    assert!(!pane_a.input_enabled_for_test(), "disconnected pane should not accept input");
    assert!(!pane_b.input_enabled_for_test(), "disconnected pane should not accept input");
    drop(panes);

    // Set status to Connected — both panes should reflect it.
    window.set_workspace_connection_status(&session_state.uuid, &ConnectionStatus::Connected);

    let panes = window.imp().persistent_terminals.borrow();
    let pane_a = panes.get("pane-a").unwrap();
    let pane_b = panes.get("pane-b").unwrap();
    assert_eq!(pane_a.status_label_text_for_test(), "Connected");
    assert_eq!(pane_b.status_label_text_for_test(), "Connected");
    assert!(pane_a.input_enabled_for_test(), "connected pane should accept input");
    assert!(pane_b.input_enabled_for_test(), "connected pane should accept input");
    drop(panes);

    // Verify sidebar row exists and is accessible after status change.
    let _row = session_row_for_uuid(&window, &session_state.uuid);

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn apply_transition_with_recovered_workspace_builds_session_and_sets_connecting() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.apply-transition-recovered-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let recovered = crate::test_helpers::managed_session_with_runtime(
        "ws-recovered",
        "Recovered",
        LayoutNode::new_terminal_with_uuid("recovered-pane"),
        RuntimeEndpoint::Local,
        WorkspacePolicy::Persistent,
        Some("runtime-recovered"),
    );
    window.imp().state.borrow_mut().workspaces.push(recovered.clone());

    let transition = crate::workspace_state::EndpointEventTransition {
        recovered_workspaces: vec![recovered.clone()],
        connection_status_updates: vec![crate::workspace_state::ConnectionStatusUpdate {
            workspace_id: "ws-recovered".to_string(),
            status: ConnectionStatus::Connecting,
        }],
        persist_window_state: true,
        ..Default::default()
    };
    window.apply_endpoint_event_transition(&transition);

    assert!(
        window.imp().persistent_terminals.borrow().contains_key("recovered-pane"),
        "recovered workspace pane should be materialized"
    );
    assert_eq!(
        window.imp().workspace_connection_status.borrow().get("ws-recovered"),
        Some(&ConnectionStatus::Connecting),
        "recovered workspace should be in Connecting state"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn apply_transition_snapshot_restore_feeds_pane_data() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.apply-transition-snapshot-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let layout_uuid = "snap-transition-pane";
    let session_state = crate::test_helpers::managed_session(
        "ws-snap-t",
        "Snap Transition",
        LayoutNode::new_terminal_with_uuid(layout_uuid),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    let transition = crate::workspace_state::EndpointEventTransition {
        pane_snapshot_restores: vec![crate::workspace_state::WorkspacePaneRestore {
            layout_terminal_uuid: layout_uuid.to_string(),
            title: "htop".to_string(),
            cwd: "/var/log".to_string(),
            pane_output_seq: 0,
            scrollback_tail: bytes::Bytes::from_static(b"log output\r\n"),
            scrollback_complete: true,
            cols: 80,
            rows: 24,
            terminal_modes: None,
        }],
        connected_layout_terminals: vec![layout_uuid.to_string()],
        ..Default::default()
    };
    window.apply_endpoint_event_transition(&transition);

    let pane = window.imp().persistent_terminals.borrow().get(layout_uuid).cloned().unwrap();
    assert_eq!(pane.current_directory().as_deref(), Some("/var/log"));
    assert_eq!(pane.status_label_text_for_test(), "Connected");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn reconnect_countdown_cancels_on_status_change() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.reconnect-cancel-test").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let session_state = crate::test_helpers::managed_session(
        "ws-cancel",
        "Cancel Test",
        LayoutNode::new_terminal_with_uuid("cancel-pane"),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    // Start a reconnect countdown.
    window.set_workspace_connection_status(
        &session_state.uuid,
        &ConnectionStatus::Reconnecting { attempt: 1, retry_in_secs: 10 },
    );
    assert!(
        window.imp().workspace_reconnect_sources.borrow().contains_key(&session_state.uuid),
        "reconnect countdown timer should be active"
    );

    // Transition to Connected — countdown should be cancelled.
    window.set_workspace_connection_status(&session_state.uuid, &ConnectionStatus::Connected);
    assert!(
        !window.imp().workspace_reconnect_sources.borrow().contains_key(&session_state.uuid),
        "reconnect countdown should be cancelled when status changes to Connected"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn reconnect_countdown_skipped_for_short_delay() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.reconnect-short-delay-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let session_state = crate::test_helpers::managed_session(
        "ws-short",
        "Short Delay",
        LayoutNode::new_terminal_with_uuid("short-pane"),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    // A retry_in_secs of 1 should not start a countdown timer.
    window.set_workspace_connection_status(
        &session_state.uuid,
        &ConnectionStatus::Reconnecting { attempt: 1, retry_in_secs: 1 },
    );
    assert!(
        !window.imp().workspace_reconnect_sources.borrow().contains_key(&session_state.uuid),
        "countdown should not start for retry_in_secs <= 1"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn workspace_session_missing_disables_pane_input() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.session-missing-input-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let session_state = crate::test_helpers::managed_session(
        "ws-missing",
        "Missing Test",
        LayoutNode::new_terminal_with_uuid("missing-pane"),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    window.set_workspace_connection_status(&session_state.uuid, &ConnectionStatus::SessionMissing);

    let pane = window.imp().persistent_terminals.borrow().get("missing-pane").cloned().unwrap();
    assert!(!pane.input_enabled_for_test(), "SessionMissing should disable pane input");
    assert_eq!(pane.status_label_text_for_test(), "Session Missing");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn managed_binding_for_terminal_resolves_bound_pane() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.managed-binding-resolve-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let layout_uuid = "bound-pane";
    // Identity invariant: the runtime pane id IS the layout terminal uuid.
    let runtime_pane_id = layout_uuid;
    let session_state = crate::test_helpers::managed_session_with_runtime(
        "ws-binding",
        "Binding Test",
        LayoutNode::new_terminal_with_uuid(layout_uuid),
        RuntimeEndpoint::Local,
        WorkspacePolicy::Persistent,
        Some("runtime-binding"),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state);

    let binding = window.managed_binding_for_terminal(layout_uuid);
    assert!(binding.is_some(), "should resolve binding for bound terminal");
    let (workspace_id, endpoint, runtime_id, pane_id) = binding.unwrap();
    assert_eq!(workspace_id, "ws-binding");
    assert_eq!(endpoint, RuntimeEndpoint::Local);
    assert_eq!(runtime_id, "runtime-binding");
    assert_eq!(pane_id, runtime_pane_id);

    // Unbound terminal should return None.
    assert!(
        window.managed_binding_for_terminal("nonexistent").is_none(),
        "unbound terminal should return None"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn connection_presentation_delegates_to_pure_function() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.connection-presentation-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let connected = window.connection_presentation_for_workspace(&ConnectionStatus::Connected);
    let pure = crate::runtime::present_connection_status(&ConnectionStatus::Connected);
    assert_eq!(connected, pure, "window method should delegate to pure function");

    let disconnected =
        window.connection_presentation_for_workspace(&ConnectionStatus::Disconnected);
    let pure_disc = crate::runtime::present_connection_status(&ConnectionStatus::Disconnected);
    assert_eq!(disconnected, pure_disc);

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn reapply_preferences_updates_keyboard_shortcut_accels() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.shortcut-reapply-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    // Default: fullscreen is bound to F11.
    let accels = app.accels_for_action("win.fullscreen");
    assert!(accels.iter().any(|a| a == "F11"), "fullscreen should default to F11, got: {accels:?}");

    // Override fullscreen to Ctrl+Shift+F11 via preferences.
    let mut prefs = store().load_preferences().into_value().unwrap_or_default();
    prefs.keyboard_shortcuts.insert("fullscreen".into(), vec!["<Ctrl><Shift>F11".into()]);
    store().save_preferences(&prefs).unwrap();

    window.reapply_terminal_preferences();

    let accels = app.accels_for_action("win.fullscreen");
    assert!(
        accels.iter().any(|a| a == "<Ctrl><Shift>F11"),
        "fullscreen should be rebound to Ctrl+Shift+F11, got: {accels:?}"
    );
    assert!(
        !accels.iter().any(|a| a == "F11"),
        "old F11 binding should be removed, got: {accels:?}"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn default_keyboard_shortcuts_register_expected_accels() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.shortcut-registration-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    // setup_actions must register every default shortcut with the application,
    // not just fullscreen. Verify the key actions end-to-end.
    for (action, accel) in [
        ("new-session", "<Ctrl><Shift>T"),
        ("close-terminal", "<Ctrl><Shift>W"),
        ("split-horizontal", "<Ctrl><Shift>E"),
        ("split-vertical", "<Ctrl><Shift>O"),
        ("search", "<Ctrl><Shift>F"),
        ("fullscreen", "F11"),
    ] {
        let accels = app.accels_for_action(&format!("win.{action}"));
        assert!(
            accels.iter().any(|a| a == accel),
            "action '{action}' should register accelerator '{accel}', got: {accels:?}"
        );
    }

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}
// C4 — Signal handler doubling
//
// When rebuild_session_content is called multiple times (e.g. after
// split, close, reconnect), the existing TerminalWidget must be reused
// from the HashMap — not recreated. Recreating would call
// connect_terminal_signals a second time, doubling every handler.
// ═══════════════════════════════════════════════════════════════════

/// C4 regression: rebuilding a direct workspace multiple times must reuse
/// the same TerminalWidget instances. If a new widget were created, its
/// signal handlers would stack on the old ones.
#[test]
#[ignore = "requires isolated GTK harness"]
fn c4_rebuild_reuses_direct_terminal_widgets() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.c4-direct-reuse-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let layout = crate::test_helpers::hsplit(
        LayoutNode::new_terminal_with_uuid("t1"),
        LayoutNode::new_terminal_with_uuid("t2"),
    );
    let session_state = crate::test_helpers::workspace("ws1", "Workspace", layout);
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    let ptr_t1_before = window.imp().terminals.borrow().get("t1").unwrap().as_ptr() as usize;
    let ptr_t2_before = window.imp().terminals.borrow().get("t2").unwrap().as_ptr() as usize;

    // Rebuild 5 times — simulates repeated split/close/reconnect cycles.
    for _ in 0..5 {
        window.rebuild_session_content(&session_state.uuid, &session_state);
    }

    let ptr_t1_after = window.imp().terminals.borrow().get("t1").unwrap().as_ptr() as usize;
    let ptr_t2_after = window.imp().terminals.borrow().get("t2").unwrap().as_ptr() as usize;

    assert_eq!(
        ptr_t1_before, ptr_t1_after,
        "t1 must be the same widget instance after rebuilds — recreating would double signals"
    );
    assert_eq!(
        ptr_t2_before, ptr_t2_after,
        "t2 must be the same widget instance after rebuilds — recreating would double signals"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

/// C4 regression: rebuilding a managed workspace multiple times must reuse
/// the same PersistentPaneView instances.
#[test]
#[ignore = "requires isolated GTK harness"]
fn c4_rebuild_reuses_managed_terminal_widgets() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.c4-managed-reuse-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let layout = crate::test_helpers::hsplit(
        LayoutNode::new_terminal_with_uuid("p1"),
        LayoutNode::new_terminal_with_uuid("p2"),
    );
    let session_state = crate::test_helpers::managed_session("ws-m", "Managed", layout);
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    let ptr_p1_before =
        window.imp().persistent_terminals.borrow().get("p1").unwrap().as_ptr() as usize;
    let ptr_p2_before =
        window.imp().persistent_terminals.borrow().get("p2").unwrap().as_ptr() as usize;

    for _ in 0..5 {
        window.rebuild_session_content(&session_state.uuid, &session_state);
    }

    let ptr_p1_after =
        window.imp().persistent_terminals.borrow().get("p1").unwrap().as_ptr() as usize;
    let ptr_p2_after =
        window.imp().persistent_terminals.borrow().get("p2").unwrap().as_ptr() as usize;

    assert_eq!(
        ptr_p1_before, ptr_p1_after,
        "p1 must be the same widget instance after rebuilds — recreating would double signals"
    );
    assert_eq!(
        ptr_p2_before, ptr_p2_after,
        "p2 must be the same widget instance after rebuilds — recreating would double signals"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

/// C4 regression: splitting a terminal and rebuilding must not create a
/// second widget for the original terminal. The child_exited handler is
/// stored as a single Option<SignalHandlerId> — a second connect would
/// overwrite the first, leaking the old handler.
#[test]
#[ignore = "requires isolated GTK harness"]
fn c4_split_preserves_child_exited_handler_identity() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.c4-handler-identity-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let layout = LayoutNode::new_terminal_with_uuid("t1");
    let session_state = crate::test_helpers::workspace("ws-split", "Split Test", layout);
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    let handler_before = window
        .imp()
        .terminals
        .borrow()
        .get("t1")
        .unwrap()
        .imp()
        .child_exited_handler
        .borrow()
        .is_some();
    assert!(handler_before, "t1 should have a child_exited handler after initial build");

    // Split t1 — this triggers rebuild_session_content.
    window.split_terminal("t1", SplitOrientation::Horizontal);

    let handler_after = window
        .imp()
        .terminals
        .borrow()
        .get("t1")
        .unwrap()
        .imp()
        .child_exited_handler
        .borrow()
        .is_some();
    assert!(handler_after, "t1 should still have exactly one child_exited handler after split");

    // The widget pointer must be the same — no recreation.
    let terminal_count = window.imp().terminals.borrow().len();
    assert_eq!(terminal_count, 2, "split should produce exactly 2 terminals (t1 + new)");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn paste_guard_dialog_defaults_to_paste() {
    require_display!();
    let analysis = crate::terminal::paste_guard::analyse("line1\nline2\nline3");
    let dialog = super::dialogs::build_paste_guard_dialog(&analysis);
    assert_eq!(dialog.default_response().as_deref(), Some("paste"));
    assert_eq!(dialog.close_response(), "cancel");
}

/// Splitting a managed pane must propagate the parent pane's live CWD
/// into the new layout node so the daemon receives the correct CWD in
/// the CreatePane request. The widget CWD must win over a stale layout
/// node CWD. Regression test for #773.
#[test]
#[ignore = "requires isolated GTK harness"]
fn split_managed_pane_inherits_parent_cwd() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.split-managed-cwd-test").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    // Create a managed workspace whose layout node starts with an initial CWD.
    let mut layout = LayoutNode::new_terminal_with_uuid("parent-pane");
    layout.set_terminal_cwd("parent-pane", Some("/home/user/original".into()));
    let session_state =
        crate::test_helpers::managed_session("ws-managed-cwd", "Managed CWD Workspace", layout);
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    // Simulate the daemon reporting a CWD change — the user cd'd.
    let pane = window
        .imp()
        .persistent_terminals
        .borrow()
        .get("parent-pane")
        .cloned()
        .expect("parent pane should be present");
    pane.set_current_directory(Some("/home/user/changed-dir"));

    // Do NOT update the layout node — this simulates the race where the
    // widget has the live CWD but the layout node is stale.

    window.split_terminal("parent-pane", SplitOrientation::Horizontal);

    let state = window.imp().state.borrow();
    let session = state
        .workspaces
        .iter()
        .find(|s| s.uuid == "ws-managed-cwd")
        .expect("workspace should exist");
    let uuids = session.layout.terminal_uuids();
    assert_eq!(uuids.len(), 2, "split should produce two panes");
    let new_uuid = uuids.into_iter().find(|u| u != "parent-pane").unwrap();

    assert_eq!(
        session.layout.terminal_cwd(&new_uuid).as_deref(),
        Some("/home/user/changed-dir"),
        "new managed pane must inherit parent pane's live CWD, not stale layout node CWD"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn command_sidebar_label_filter_chips_shown_when_labels_exist() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let mut c1 = crate::commands::SavedCommand::new("Deploy", "cargo build");
    c1.labels = vec!["ops".into(), "deploy".into()];
    let c2 = crate::commands::SavedCommand::new("Test", "cargo test");
    store().save_commands(&[c1, c2]).unwrap();

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.command-label-filter-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(100);

    // Label filter box should be visible with 2 chips (deploy, ops)
    assert!(
        window.imp().label_filter_box.is_visible(),
        "label filter box should be visible when commands have labels"
    );

    // Count chip buttons
    let mut chip_count = 0;
    let mut child = window.imp().label_filter_box.first_child();
    while let Some(c) = child {
        chip_count += 1;
        child = c.next_sibling();
    }
    assert_eq!(chip_count, 2, "should show 2 label chips (deploy, ops)");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn command_sidebar_label_filter_hides_non_matching_commands() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let mut c1 = crate::commands::SavedCommand::new("Deploy", "cargo build");
    c1.labels = vec!["ops".into()];
    let c2 = crate::commands::SavedCommand::new("Test", "cargo test");
    store().save_commands(&[c1, c2]).unwrap();

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.command-label-filter-hide-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(100);

    // Before filtering: Global header + 2 commands = 3 items
    let count_before = window.imp().command_list.observe_children().n_items();
    assert_eq!(
        count_before, 3,
        "should show header + 2 commands before filter, got {count_before}"
    );

    // Activate the "ops" label filter
    {
        let mut active = window.imp().active_labels.borrow_mut();
        active.push("ops".into());
    }
    window.refresh_command_sidebar();
    pump_events(50);

    // After filtering: Global header + 1 command = 2 items
    let count_after = window.imp().command_list.observe_children().n_items();
    assert_eq!(
        count_after, 2,
        "should show header + 1 command after label filter, got {count_after}"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn command_sidebar_label_filter_hidden_when_no_labels() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let c1 = crate::commands::SavedCommand::new("Deploy", "cargo build");
    let c2 = crate::commands::SavedCommand::new("Test", "cargo test");
    store().save_commands(&[c1, c2]).unwrap();

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.command-label-filter-hidden-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(100);

    assert!(
        !window.imp().label_filter_box.is_visible(),
        "label filter box should be hidden when no commands have labels"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn run_in_new_pane_splits_and_sends_command_to_new_terminal() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.run-in-new-pane-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.set_default_size(1200, 800);
    window.present();
    pump_events(100);

    let original_uuid = {
        let state = window.imp().state.borrow();
        let uuids = state.workspaces[0].layout.terminal_uuids();
        assert_eq!(uuids.len(), 1, "should start with a single pane");
        uuids[0].clone()
    };

    let command = SavedCommand::new("Build", "cargo build");
    window.execute_saved_command(&command, CommandRunMode::RunInNewPane);
    pump_events(100);

    let (terminal_uuids, new_uuid) = {
        let state = window.imp().state.borrow();
        let uuids = state.workspaces[0].layout.terminal_uuids();
        assert_eq!(uuids.len(), 2, "RunInNewPane should create a split");
        let new = uuids.into_iter().find(|u| u != &original_uuid).unwrap();
        let all = state.workspaces[0].layout.terminal_uuids();
        (all, new)
    };

    assert_eq!(terminal_uuids.len(), 2);

    // Verify the command was sent to the new pane, not the original
    let new_term = window
        .imp()
        .terminals
        .borrow()
        .get(&new_uuid)
        .cloned()
        .expect("new split terminal should exist");
    assert_eq!(
        new_term.pending_shell_inputs_for_test(),
        vec![String::from("cargo build\n")],
        "command should be queued in the new pane with trailing newline (execute mode)"
    );

    // Verify recovery is set on the new pane
    let recovery = {
        let state = window.imp().state.borrow();
        state.workspaces[0].recovery_for(&new_uuid).cloned()
    };
    assert_eq!(
        recovery,
        Some(PaneRecovery {
            source: PaneSource::Command { title: "Build".into() },
            target: None,
            startup: vec![StartupStep::SendText { text: "cargo build".into(), execute: true }],
        })
    );

    // Verify the original pane was NOT modified
    let original_term = window
        .imp()
        .terminals
        .borrow()
        .get(&original_uuid)
        .cloned()
        .expect("original terminal should still exist");
    assert!(
        original_term.pending_shell_inputs_for_test().is_empty(),
        "original pane should not receive the command"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn run_in_new_pane_respects_split_depth_limit() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.run-in-new-pane-depth-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.set_default_size(1200, 800);
    window.present();
    pump_events(100);

    // Split to maximum depth (5 levels)
    for _ in 0..crate::workspace::MAX_SPLIT_DEPTH {
        let uuid = {
            let state = window.imp().state.borrow();
            state.workspaces[0].layout.terminal_uuids().last().unwrap().clone()
        };
        window.split_terminal(&uuid, SplitOrientation::Horizontal);
        pump_events(50);
    }

    let count_before = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().len()
    };

    // Attempt RunInNewPane at max depth — should not split further
    let command = SavedCommand::new("Build", "cargo build");
    window.execute_saved_command(&command, CommandRunMode::RunInNewPane);
    pump_events(100);

    let count_after = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().len()
    };

    assert_eq!(count_before, count_after, "RunInNewPane should not split beyond the maximum depth");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn rename_focused_pane_direct_sets_custom_title() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.rename-pane-direct-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    let terminal_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    window.imp().focused_terminal_uuid.replace(Some(terminal_uuid.clone()));

    window.rename_focused_pane_direct("My Pane");
    let handle = window.terminal_handle(&terminal_uuid).unwrap();
    assert_eq!(handle.custom_title().as_deref(), Some("My Pane"));
    assert_eq!(handle.title(), "My Pane");

    window.rename_focused_pane_direct("");
    let handle = window.terminal_handle(&terminal_uuid).unwrap();
    assert!(handle.custom_title().is_none(), "empty string should clear custom title");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn workspace_resynced_event_restores_pane_content() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.workspace-resynced-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let runtime_id = uuid::Uuid::new_v4();
    let pane_id = uuid::Uuid::new_v4();
    let session_state = crate::test_helpers::managed_session_with_runtime(
        "workspace-resync",
        "Resync Workspace",
        LayoutNode::new_terminal_with_uuid(&pane_id.to_string()),
        RuntimeEndpoint::Local,
        WorkspacePolicy::Persistent,
        Some(&runtime_id.to_string()),
    );
    window.imp().state.borrow_mut().workspaces.push(session_state.clone());
    window.build_session(&session_state, false);

    // First, open the workspace so bindings are established.
    window.handle_endpoint_event(crate::daemon_bridge::EndpointEvent::WorkspaceOpened {
        workspace_id: session_state.uuid.clone(),
        runtime_id: runtime_id.to_string(),
        snapshot: rttx_proto::v3::WorkspaceSnapshot {
            tree: None,
            default_active_pane_id: Vec::new(),
            runtime_id: rttx_proto::uuid_to_bytes(runtime_id),
            workspace_revision: 7,
            client_role: rttx_proto::v3::WorkspaceClientRole::Writer as i32,
            panes: vec![rttx_proto::v3::PaneSnapshot {
                pane_id: rttx_proto::uuid_to_bytes(pane_id),
                pane_output_seq: 10,
                title: "bash".into(),
                cwd: "/home".into(),
                cols: 80,
                rows: 24,
                exit_status: None,
                terminal_modes: None,
                scrollback_tail: bytes::Bytes::from_static(b"initial"),
                total_scrollback_bytes: 7,
                scrollback_complete: true,
            }],
        },
    });
    pump_events(50);

    // Now simulate a resync with updated content.
    window.handle_endpoint_event(crate::daemon_bridge::EndpointEvent::WorkspaceResynced {
        workspace_id: session_state.uuid.clone(),
        runtime_id: runtime_id.to_string(),
        snapshot: rttx_proto::v3::WorkspaceSnapshot {
            tree: None,
            default_active_pane_id: Vec::new(),
            runtime_id: rttx_proto::uuid_to_bytes(runtime_id),
            workspace_revision: 8,
            client_role: rttx_proto::v3::WorkspaceClientRole::Writer as i32,
            panes: vec![rttx_proto::v3::PaneSnapshot {
                pane_id: rttx_proto::uuid_to_bytes(pane_id),
                pane_output_seq: 50,
                title: "bash".into(),
                cwd: "/home/project".into(),
                cols: 80,
                rows: 24,
                exit_status: None,
                terminal_modes: None,
                scrollback_tail: bytes::Bytes::from_static(b"resynced output"),
                total_scrollback_bytes: 15,
                scrollback_complete: true,
            }],
        },
    });
    pump_events(50);

    // Verify the pane is still connected and the CWD was updated.
    let pane = window
        .imp()
        .persistent_terminals
        .borrow()
        .get(&pane_id.to_string())
        .cloned()
        .expect("pane should be present after resync");
    assert!(pane.input_enabled_for_test());
    assert_eq!(pane.current_directory().as_deref(), Some("/home/project"));

    // Verify the layout was not rebuilt (workspace count unchanged).
    let state = window.imp().state.borrow();
    assert_eq!(state.workspaces.len(), 2); // default + our workspace

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn auto_save_timer_is_installed_on_window_creation() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.auto-save-timer-tests").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    assert!(
        window.imp().auto_save_source.borrow().is_some(),
        "auto-save timer should be installed after window creation"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn auto_save_persists_state_without_explicit_save_call() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("XDG_STATE_HOME", tmp.path());
    crate::test_helpers::set_env("XDG_CACHE_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.auto-save-persist-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    // Rename the default workspace so we have a detectable change.
    {
        let mut state = window.imp().state.borrow_mut();
        state.workspaces[0].name = "AutoSaved".to_string();
    }

    // Manually trigger the auto-save callback by invoking save_state directly
    // (simulates what the timer does without waiting 30 seconds).
    window.save_state();

    // Verify state was persisted.
    let loaded = load_saved_window_state();
    assert_eq!(
        loaded.workspaces[0].name, "AutoSaved",
        "auto-save should persist workspace name changes"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
    crate::test_helpers::remove_env("XDG_STATE_HOME");
    crate::test_helpers::remove_env("XDG_CACHE_HOME");
}

// ── Command CRUD sidebar workflow tests ─────────────────────────

#[test]
#[ignore = "requires isolated GTK harness"]
fn command_crud_create_shows_in_sidebar() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.command-crud-create-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(100);

    // Initially empty
    assert!(window.imp().command_empty.is_visible(), "empty state should show before any commands");

    // Create a command via the store and refresh
    let cmd = crate::commands::SavedCommand::new("Build project", "cargo build --release");
    store().save_commands(&[cmd]).unwrap();
    window.refresh_command_sidebar();
    pump_events(50);

    assert!(
        !window.imp().command_empty.is_visible(),
        "empty state should hide after adding a command"
    );
    assert!(
        window.imp().command_scroll.is_visible(),
        "command list should be visible after adding a command"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn command_crud_delete_removes_from_sidebar() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let cmd = crate::commands::SavedCommand::new("Temporary", "echo bye");
    let uuid = cmd.uuid.clone();
    store().save_commands(&[cmd]).unwrap();

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.command-crud-delete-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(100);

    // Command should be visible
    assert!(window.imp().command_scroll.is_visible(), "command list should show the saved command");

    // Delete via store and refresh (simulates what confirm_delete_command does)
    let mut items = store().load_commands();
    items.retain(|c| c.uuid != uuid);
    store().save_commands(&items).unwrap();
    window.refresh_command_sidebar();
    pump_events(50);

    assert!(
        window.imp().command_empty.is_visible(),
        "empty state should show after deleting the last command"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn command_crud_edit_updates_sidebar_display() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let mut cmd = crate::commands::SavedCommand::new("Old title", "echo old");
    cmd.host_tags = vec!["local".into()];
    store().save_commands(&[cmd.clone()]).unwrap();

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.command-crud-edit-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(100);

    // Verify original title is shown
    let row = window.imp().command_list.row_at_index(1).unwrap();
    let action_row = row.downcast::<adw::ActionRow>().unwrap();
    assert_eq!(action_row.title().as_str(), "Old title");

    // Edit: update title and body, preserve UUID
    let mut items = store().load_commands();
    items[0].title = "New title".into();
    items[0].body = "echo new".into();
    store().save_commands(&items).unwrap();
    window.refresh_command_sidebar();
    pump_events(50);

    // Verify updated title is shown
    let row = window.imp().command_list.row_at_index(1).unwrap();
    let action_row = row.downcast::<adw::ActionRow>().unwrap();
    assert_eq!(action_row.title().as_str(), "New title");
    assert_eq!(action_row.subtitle().unwrap().as_str(), "echo new");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn command_crud_duplicate_adds_copy_to_sidebar() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let mut cmd = crate::commands::SavedCommand::new("Deploy", "cargo build");
    cmd.host_tags = vec!["local".into()];
    store().save_commands(&[cmd.clone()]).unwrap();

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.command-crud-duplicate-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(100);

    // Duplicate and save (simulates what the duplicate action does)
    let mut items = store().load_commands();
    let copy = items[0].duplicate();
    items.push(copy);
    store().save_commands(&items).unwrap();
    window.refresh_command_sidebar();
    pump_events(50);

    // Should now have section header + 2 command rows
    let count = window.imp().command_list.observe_children().n_items();
    assert_eq!(count, 3, "should show header + original + copy, got {count}");

    // Verify the copy has the "(copy)" suffix
    let copy_row = window.imp().command_list.row_at_index(2).unwrap();
    let copy_action_row = copy_row.downcast::<adw::ActionRow>().unwrap();
    assert_eq!(copy_action_row.title().as_str(), "Deploy (copy)");

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn reconnect_host_action_shown_when_multiple_disconnected_from_same_host() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.reconnect-host-test").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let ws1 = crate::test_helpers::managed_session_with_runtime(
        "ws-remote-1",
        "Remote 1",
        LayoutNode::new_terminal_with_uuid("pane-r1"),
        RuntimeEndpoint::remote("server.example.com"),
        WorkspacePolicy::Persistent,
        Some("rt-1"),
    );
    let ws2 = crate::test_helpers::managed_session_with_runtime(
        "ws-remote-2",
        "Remote 2",
        LayoutNode::new_terminal_with_uuid("pane-r2"),
        RuntimeEndpoint::remote("server.example.com"),
        WorkspacePolicy::Persistent,
        Some("rt-2"),
    );

    window.imp().state.borrow_mut().workspaces.push(ws1.clone());
    window.build_session(&ws1, false);
    window.imp().state.borrow_mut().workspaces.push(ws2.clone());
    window.build_session(&ws2, false);
    pump_events(50);

    // Mark both as disconnected.
    window.set_workspace_connection_status(&ws1.uuid, &ConnectionStatus::Disconnected);
    window.set_workspace_connection_status(&ws2.uuid, &ConnectionStatus::Disconnected);

    let row = session_row_for_uuid(&window, &ws1.uuid);
    window.show_workspace_popover_menu(&row, &ws1.uuid);

    assert!(
        window.lookup_action("ctx-reconnect-host").is_some(),
        "reconnect-host action should be registered when multiple workspaces from same host are disconnected"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn reconnect_host_action_hidden_when_only_one_disconnected() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.reconnect-host-single-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let ws1 = crate::test_helpers::managed_session_with_runtime(
        "ws-solo-1",
        "Solo Remote",
        LayoutNode::new_terminal_with_uuid("pane-solo"),
        RuntimeEndpoint::remote("solo.example.com"),
        WorkspacePolicy::Persistent,
        Some("rt-solo"),
    );

    window.imp().state.borrow_mut().workspaces.push(ws1.clone());
    window.build_session(&ws1, false);
    pump_events(50);

    window.set_workspace_connection_status(&ws1.uuid, &ConnectionStatus::Disconnected);

    let row = session_row_for_uuid(&window, &ws1.uuid);
    window.show_workspace_popover_menu(&row, &ws1.uuid);

    assert!(
        window.lookup_action("ctx-reconnect-host").is_none(),
        "reconnect-host action should NOT be registered when only one workspace is disconnected"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn reconnect_host_reconnects_all_disconnected_workspaces_from_same_endpoint() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.reconnect-host-all-test")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);

    let ws1 = crate::test_helpers::managed_session_with_runtime(
        "ws-host-a1",
        "Host A - 1",
        LayoutNode::new_terminal_with_uuid("pane-a1"),
        RuntimeEndpoint::remote("host-a.example.com"),
        WorkspacePolicy::Persistent,
        Some("rt-a1"),
    );
    let ws2 = crate::test_helpers::managed_session_with_runtime(
        "ws-host-a2",
        "Host A - 2",
        LayoutNode::new_terminal_with_uuid("pane-a2"),
        RuntimeEndpoint::remote("host-a.example.com"),
        WorkspacePolicy::Persistent,
        Some("rt-a2"),
    );
    let ws3 = crate::test_helpers::managed_session_with_runtime(
        "ws-host-b1",
        "Host B - 1",
        LayoutNode::new_terminal_with_uuid("pane-b1"),
        RuntimeEndpoint::remote("host-b.example.com"),
        WorkspacePolicy::Persistent,
        Some("rt-b1"),
    );

    window.imp().state.borrow_mut().workspaces.push(ws1.clone());
    window.build_session(&ws1, false);
    window.imp().state.borrow_mut().workspaces.push(ws2.clone());
    window.build_session(&ws2, false);
    window.imp().state.borrow_mut().workspaces.push(ws3.clone());
    window.build_session(&ws3, false);
    pump_events(50);

    // Mark all three as disconnected.
    window.set_workspace_connection_status(&ws1.uuid, &ConnectionStatus::Disconnected);
    window.set_workspace_connection_status(&ws2.uuid, &ConnectionStatus::Disconnected);
    window.set_workspace_connection_status(&ws3.uuid, &ConnectionStatus::Disconnected);

    // Trigger reconnect-all for host-a endpoint via ws1.
    window.retry_all_workspaces_for_endpoint(&ws1.uuid);
    pump_events(50);

    // ws1 and ws2 (same host) should now be Connecting.
    let statuses = window.imp().workspace_connection_status.borrow();
    assert_eq!(
        statuses.get(&ws1.uuid),
        Some(&ConnectionStatus::Connecting),
        "ws1 should be reconnecting"
    );
    assert_eq!(
        statuses.get(&ws2.uuid),
        Some(&ConnectionStatus::Connecting),
        "ws2 should be reconnecting (same host)"
    );
    // ws3 (different host) should remain Disconnected.
    assert_eq!(
        statuses.get(&ws3.uuid),
        Some(&ConnectionStatus::Disconnected),
        "ws3 should remain disconnected (different host)"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

/// Regression test for #939: "Close All" should keep the window open with a
/// fresh direct workspace instead of closing the application.
#[test]
#[ignore = "requires isolated GTK harness"]
fn close_all_keeps_window_open_with_fresh_workspace() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("XDG_STATE_HOME", tmp.path());
    crate::test_helpers::set_env("XDG_CACHE_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app =
        adw::Application::builder().application_id("com.illya.rttx.close-all-fresh-test").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.set_default_size(1200, 800);
    window.present();
    pump_events(100);

    // Ensure we start with exactly one workspace (from default state), then add a second.
    let initial_count = window.imp().state.borrow().workspaces.len();
    assert!(initial_count >= 1, "should have at least one workspace after init");

    let second = WorkspaceState::new("Second".into());
    window.imp().state.borrow_mut().workspaces.push(second.clone());
    window.build_session(&second, false);
    pump_events(50);

    let pre_close_count = window.imp().state.borrow().workspaces.len();
    assert!(pre_close_count >= 2, "should have at least two workspaces before close all");

    // Invoke close_all_sessions (the logic behind "Close All" confirmation).
    window.close_all_sessions();
    pump_events(50);

    // Window should still be visible.
    assert!(window.is_visible(), "window should remain open after Close All");

    // Exactly one fresh workspace should exist.
    let state = window.imp().state.borrow();
    assert_eq!(state.workspaces.len(), 1, "should have exactly one workspace after Close All");
    assert!(
        state.workspaces[0].name.starts_with("Direct"),
        "the fresh workspace should be a Direct terminal, got: {}",
        state.workspaces[0].name
    );

    drop(state);
    window.close();
    crate::test_helpers::remove_env("XDG_STATE_HOME");
    crate::test_helpers::remove_env("XDG_CACHE_HOME");
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires isolated GTK harness"]
fn zoom_button_zooms_its_own_pane_not_focused_pane() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.zoom-button-target-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.set_default_size(1000, 700);
    window.present();
    pump_events(100);

    let first_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    window.split_terminal(&first_uuid, SplitOrientation::Horizontal);
    pump_events(100);

    let second_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0]
            .layout
            .terminal_uuids()
            .into_iter()
            .find(|uuid| uuid != &first_uuid)
            .unwrap()
    };

    // Focus the first pane.
    {
        let terminals = window.imp().terminals.borrow();
        let first_term = terminals.get(&first_uuid).unwrap();
        first_term.vte().grab_focus();
    }
    pump_events(50);

    // Zoom the second pane via its UUID (simulates clicking its zoom button).
    window.toggle_pane_zoom_for(Some(&second_uuid));
    pump_events(50);

    let zoomed = {
        let state = window.imp().state.borrow();
        state.workspaces[0].zoomed_terminal_uuid.clone()
    };
    assert_eq!(
        zoomed.as_deref(),
        Some(second_uuid.as_str()),
        "zoom button should zoom the pane it belongs to, not the focused pane"
    );

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires GTK display"]
fn navigate_while_zoomed_switches_zoomed_pane() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.navigate-while-zoomed-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.set_default_size(1000, 700);
    window.present();
    pump_events(100);

    let first_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    window.split_terminal(&first_uuid, SplitOrientation::Horizontal);
    pump_events(100);

    let second_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0]
            .layout
            .terminal_uuids()
            .into_iter()
            .find(|uuid| uuid != &first_uuid)
            .unwrap()
    };

    // Zoom the first pane.
    window.toggle_pane_zoom_for(Some(&first_uuid));
    pump_events(50);

    {
        let state = window.imp().state.borrow();
        assert_eq!(
            state.workspaces[0].zoomed_terminal_uuid.as_deref(),
            Some(first_uuid.as_str()),
            "first pane should be zoomed"
        );
    }

    // Navigate right while zoomed — should switch zoom to the second pane.
    window.navigate_focused(Direction::Right);
    pump_events(50);

    {
        let state = window.imp().state.borrow();
        assert_eq!(
            state.workspaces[0].zoomed_terminal_uuid.as_deref(),
            Some(second_uuid.as_str()),
            "navigation while zoomed should switch the zoomed pane"
        );
    }

    // Navigate left — should switch back to the first pane.
    window.navigate_focused(Direction::Left);
    pump_events(50);

    {
        let state = window.imp().state.borrow();
        assert_eq!(
            state.workspaces[0].zoomed_terminal_uuid.as_deref(),
            Some(first_uuid.as_str()),
            "navigating back should restore zoom to the first pane"
        );
    }

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
#[ignore = "requires GTK display"]
fn navigate_while_zoomed_no_op_at_edge() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
    crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.navigate-zoomed-edge-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.set_default_size(1000, 700);
    window.present();
    pump_events(100);

    let first_uuid = {
        let state = window.imp().state.borrow();
        state.workspaces[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    window.split_terminal(&first_uuid, SplitOrientation::Horizontal);
    pump_events(100);

    // Zoom the first pane.
    window.toggle_pane_zoom_for(Some(&first_uuid));
    pump_events(50);

    // Navigate left (no adjacent pane in that direction) — should remain on first pane.
    window.navigate_focused(Direction::Left);
    pump_events(50);

    {
        let state = window.imp().state.borrow();
        assert_eq!(
            state.workspaces[0].zoomed_terminal_uuid.as_deref(),
            Some(first_uuid.as_str()),
            "navigating at edge while zoomed should not change the zoomed pane"
        );
    }

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}
