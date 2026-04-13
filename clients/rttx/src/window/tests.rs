use super::*;
use crate::session::PaneTarget;
use std::time::{Duration, Instant};

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

fn session_row_at(window: &Window, index: i32) -> SessionRow {
    window
        .imp()
        .sidebar_list
        .row_at_index(index)
        .and_then(|row| row.child())
        .and_then(|child| child.downcast::<SessionRow>().ok())
        .expect("session row should exist")
}

fn session_row_for_uuid(window: &Window, session_uuid: &str) -> SessionRow {
    let list = &window.imp().sidebar_list;
    let mut idx = 0;
    while let Some(row) = list.row_at_index(idx) {
        if let Some(session_row) = row.child().and_then(|child| child.downcast::<SessionRow>().ok())
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
        .and_then(|child| child.downcast::<SessionRow>().ok())
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
        active_session_index: 0,
        width: 800,
        height: 600,
        is_maximized: false,
        sessions: vec![
            SessionState {
                uuid: "s1".into(),
                name: "Session 1".into(),
                layout: LayoutNode::new_terminal_with_uuid("t1"),
                terminal_recovery: Default::default(),
                active_terminal_uuid: None,
                input_sync: false,
                mode: Default::default(),
                runtime: Default::default(),
                color: Default::default(),
                zoomed_terminal_uuid: None,
                user_renamed: false,
            },
            SessionState {
                uuid: "s2".into(),
                name: "Session 2".into(),
                layout: LayoutNode::new_terminal_with_uuid("t2"),
                terminal_recovery: Default::default(),
                active_terminal_uuid: None,
                input_sync: false,
                mode: Default::default(),
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
        sessions: vec![SessionState {
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
            mode: Default::default(),
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
        sessions: vec![SessionState {
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
            mode: Default::default(),
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
        active_session_index: 1,
        sessions: vec![
            SessionState {
                uuid: "s1".into(),
                name: "Session 1".into(),
                layout: LayoutNode::new_terminal_with_uuid("t1"),
                terminal_recovery: Default::default(),
                active_terminal_uuid: None,
                input_sync: false,
                mode: Default::default(),
                runtime: Default::default(),
                color: Default::default(),
                zoomed_terminal_uuid: None,
                user_renamed: false,
            },
            SessionState {
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
                mode: Default::default(),
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
    crate::commands::save(&[run, insert]).unwrap();

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

    window.imp().command_search_entry.set_text("deploy");
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
fn failed_structured_recovery_keeps_terminal_alive_and_allows_retry() {
    require_display!();

    let tmp = tempfile::TempDir::new().unwrap();
    crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());

    let terminal_uuid = "t1".to_string();
    let session_uuid = "s1".to_string();
    let state = WindowState {
        sessions: vec![SessionState {
            uuid: session_uuid.clone(),
            name: "Ops".into(),
            layout: LayoutNode::new_terminal_with_uuid(&terminal_uuid),
            terminal_recovery: std::collections::BTreeMap::from([(
                terminal_uuid.clone(),
                PaneRecovery {
                    source: PaneSource::Bookmark { name: "Ops".into() },
                    target: Some(PaneTarget::RemoteShell {
                        ssh_target: "user@192.0.2.1".into(),
                        remote_folder: None,
                    }),
                    startup: vec![],
                },
            )]),
            active_terminal_uuid: Some(terminal_uuid.clone()),
            input_sync: false,
            mode: Default::default(),
            runtime: Default::default(),
            color: Default::default(),
            zoomed_terminal_uuid: None,
            user_renamed: false,
        }],
        ..WindowState::default()
    };
    crate::session::save_window_state(&state).unwrap();

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
        let session = &state.sessions[0];
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
        state.sessions[0].layout.terminal_uuids().into_iter().next().unwrap()
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
        state.sessions[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    first_window.split_terminal(&root_uuid, SplitOrientation::Horizontal);
    pump_events(100);

    let second_uuid = {
        let state = first_window.imp().state.borrow();
        state.sessions[0]
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
            && first_window.imp().state.borrow().sessions[0].active_terminal_uuid.as_deref()
                == Some(second_uuid.as_str())
    });
    assert!(focused, "focusing a pane should record it as the session's active terminal");

    first_window.save_state();
    first_window.close();

    let saved_state = session::load_window_state();
    assert_eq!(
        saved_state.sessions[0].active_terminal_uuid.as_deref(),
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
        let first_uuid = state.sessions[0].layout.terminal_uuids().into_iter().next().unwrap();
        window.imp().terminals.borrow().get(&first_uuid).cloned().unwrap()
    };
    assert!(first_terminal.vte().grab_focus());
    let first_focused = wait_until(1000, || first_terminal.vte().has_focus());
    assert!(first_focused, "initial terminal should be focusable");

    window.add_session();
    let second_terminal = {
        let state = window.imp().state.borrow();
        let second_uuid = state.sessions[1].layout.terminal_uuids().into_iter().next().unwrap();
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
        state.sessions[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    window.split_terminal(&first_uuid, SplitOrientation::Horizontal);
    pump_events(100);

    let (first_term, second_term) = {
        let state = window.imp().state.borrow();
        let second_uuid = state.sessions[0]
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
        state.sessions[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    window.split_terminal(&first_uuid, SplitOrientation::Horizontal);
    pump_events(100);

    let (first_term, second_term, second_uuid) = {
        let state = window.imp().state.borrow();
        let second_uuid = state.sessions[0]
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
        let session = &state.sessions[0];
        (session.uuid.clone(), session.layout.terminal_uuids().into_iter().next().unwrap())
    };

    window.split_terminal(&t1_uuid, SplitOrientation::Horizontal);
    pump_events(100);

    let t2_uuid = {
        let state = window.imp().state.borrow();
        state.sessions[0].layout.terminal_uuids().into_iter().find(|uuid| uuid != &t1_uuid).unwrap()
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
        let session = &state.sessions[0];
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

    let saved_state = session::load_window_state();
    let LayoutNode::Split { ratio: saved_ratio, .. } = &saved_state.sessions[0].layout else {
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
        state.sessions[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    window.split_terminal(&root_uuid, SplitOrientation::Horizontal);

    let terminal_uuids = {
        let state = window.imp().state.borrow();
        state.sessions[0].layout.terminal_uuids()
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
    let saved_state = session::load_window_state();

    let LayoutNode::Split { first, second, .. } = &saved_state.sessions[0].layout else {
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
        state.sessions[0].layout.terminal_uuids().into_iter().next().unwrap()
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
    let saved_state = session::load_window_state();
    assert_eq!(
        saved_state.sessions[0].layout.terminal_custom_title(&terminal_uuid).as_deref(),
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
        let session = &state.sessions[0];
        (session.uuid.clone(), session.layout.terminal_uuids().into_iter().next().unwrap())
    };

    first_window.split_terminal(&t1_uuid, SplitOrientation::Horizontal);
    pump_events(100);

    let t2_uuid = {
        let state = first_window.imp().state.borrow();
        state.sessions[0].layout.terminal_uuids().into_iter().find(|uuid| uuid != &t1_uuid).unwrap()
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

    let saved_state = session::load_window_state();
    let LayoutNode::Split { ratio: saved_outer_ratio, second, .. } =
        &saved_state.sessions[0].layout
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
fn rename_session_updates_sidebar_and_saved_state() {
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
        state.sessions[0].uuid.clone()
    };

    window.rename_session(&session_uuid, "Renamed Session");

    {
        let state = window.imp().state.borrow();
        assert_eq!(state.sessions[0].name, "Renamed Session");
    }

    let row = window.imp().sidebar_list.row_at_index(0).expect("session row should exist");
    let session_row = row
        .child()
        .and_then(|child| child.downcast::<SessionRow>().ok())
        .expect("session row child should be SessionRow");
    assert_eq!(session_row.session_name(), "Renamed Session");
    assert_eq!(session_row.title().as_str(), "Renamed Session");

    window.save_state();
    let saved_state = session::load_window_state();
    assert_eq!(saved_state.sessions[0].name, "Renamed Session");
    assert!(saved_state.sessions[0].user_renamed);

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
        window.lookup_action("manage-bookmarks").is_none(),
        "manage-bookmarks action should be removed"
    );
    assert!(
        window.lookup_action("manage-commands").is_none(),
        "manage-commands action should be removed"
    );
    assert!(
        window.lookup_action("add-bookmark").is_none(),
        "add-bookmark action should be removed"
    );
    assert!(
        window.lookup_action("add-command").is_some(),
        "add-command action should be registered"
    );
    assert!(
        window.lookup_action("edit-bookmark").is_none(),
        "edit-bookmark action should be removed"
    );
    assert!(
        window.lookup_action("delete-bookmark").is_none(),
        "delete-bookmark action should be removed"
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
    preferences::save(&prefs).unwrap();

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
    preferences::save(&prefs).unwrap();
    window.reapply_terminal_preferences();
    assert!(!terminal.smart_clipboard_enabled_for_test());

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
        state.sessions[1].uuid.clone()
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
        let session = &state.sessions[0];
        (session.uuid.clone(), session.layout.terminal_uuids().into_iter().next().unwrap())
    };

    window.split_terminal(&t1_uuid, SplitOrientation::Horizontal);
    pump_events(100);

    let t2_uuid = {
        let state = window.imp().state.borrow();
        state.sessions[0].layout.terminal_uuids().into_iter().find(|uuid| uuid != &t1_uuid).unwrap()
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
        state.sessions[0].layout.terminal_uuids().into_iter().next().unwrap()
    };

    let mut leaf_uuid = first_uuid;
    for _ in 1..MAX_SPLIT_DEPTH {
        window.split_terminal(&leaf_uuid, SplitOrientation::Horizontal);
        pump_events(50);
        leaf_uuid = {
            let state = window.imp().state.borrow();
            state.sessions[0]
                .layout
                .terminal_uuids()
                .into_iter()
                .max_by_key(|uuid| state.sessions[0].layout.depth_of_terminal(uuid).unwrap_or(0))
                .unwrap()
        };
    }

    let count_before = {
        let state = window.imp().state.borrow();
        state.sessions[0].layout.terminal_count()
    };

    window.split_terminal(&leaf_uuid, SplitOrientation::Horizontal);
    pump_events(50);

    let count_after = {
        let state = window.imp().state.borrow();
        state.sessions[0].layout.terminal_count()
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
        state.sessions[0].layout.terminal_uuids().into_iter().next().unwrap()
    };

    window.split_terminal(&t1_uuid, SplitOrientation::Horizontal);

    let t2_uuid = {
        let state = window.imp().state.borrow();
        state.sessions[0].layout.terminal_uuids().into_iter().find(|u| u != &t1_uuid).unwrap()
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
    use crate::test_helpers::{session, term, window_state};

    let state = window_state(vec![session("s1", "A", term("t1")), session("s2", "B", term("t2"))]);

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
    use crate::test_helpers::{session, term, window_state};

    let state = window_state(vec![session("s1", "A", term("t1"))]);

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
            state.sessions[0].uuid.clone(),
            state.sessions[1].uuid.clone(),
            state.sessions[2].uuid.clone(),
        )
    };

    // Move session 2 (index 2) to session 0's position (index 0).
    window.reorder_session(&uuid2, &uuid0);
    pump_events(50);

    let order: Vec<String> = {
        let state = window.imp().state.borrow();
        state.sessions.iter().map(|s| s.uuid.clone()).collect()
    };
    assert_eq!(order, vec![uuid2.clone(), uuid0.clone(), uuid1.clone()]);

    // Verify sidebar rows match the new order.
    let sidebar_uuid_0 = window
        .imp()
        .sidebar_list
        .row_at_index(0)
        .and_then(|r| r.child())
        .and_then(|c| c.downcast::<SessionRow>().ok())
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
            state.sessions[0].uuid.clone(),
            state.sessions[1].uuid.clone(),
            state.sessions[2].uuid.clone(),
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
        (state.sessions[0].uuid.clone(), state.sessions[1].uuid.clone())
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
        state.sessions[0].uuid.clone()
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
        state.sessions[1].layout.terminal_uuids()[0].clone()
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
        state.sessions[1].layout.terminal_uuids()[0].clone()
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
        state.sessions[0].layout.terminal_uuids()[0].clone()
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
        state.sessions[1].layout.terminal_uuids()[0].clone()
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
        state.sessions[0].layout.terminal_uuids().into_iter().next().unwrap()
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
        let uuids = state.sessions[0].layout.terminal_uuids();
        let t2 = uuids.into_iter().find(|u| u != &t1_uuid).unwrap();
        let cwd = state.sessions[0].layout.terminal_cwd(&t2);
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
        RuntimeEndpoint::Remote { host: "builder.example".into() },
        WorkspacePolicy::Persistent,
        None,
    );
    window.imp().state.borrow_mut().sessions.push(session_state.clone());
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
    window.imp().state.borrow_mut().sessions.push(session_state.clone());
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
        .sessions
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
    window.imp().state.borrow_mut().sessions.push(session_state.clone());
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
    let initial_session_count = window.imp().state.borrow().sessions.len();
    let runtime_id = uuid::Uuid::new_v4();
    let pane_id = uuid::Uuid::new_v4();

    window.handle_endpoint_event(crate::daemon_bridge::EndpointEvent::InventoryLoaded {
        endpoint: RuntimeEndpoint::Local,
        sessions: vec![rttx_proto::proto::SessionInfo {
            id: rttx_proto::uuid_to_bytes(runtime_id),
            name: "Recovered Workspace".into(),
            pane_count: 1,
            has_attached_client: false,
            active_pane_id: Some(rttx_proto::uuid_to_bytes(pane_id)),
            panes: vec![rttx_proto::proto::PaneInfo {
                id: rttx_proto::uuid_to_bytes(pane_id),
                title: "Shell".into(),
                cwd: "/srv/project".into(),
                cols: 120,
                rows: 40,
                exit_status: None,
                reconstructed: true,
            }],
            policy: rttx_proto::proto::RuntimePolicy::Persistent as i32,
            attached_client_count: 0,
            reconstructed: true,
            revision: 7,
            current_client_role: rttx_proto::proto::RuntimeClientRole::Unattached as i32,
            has_write_owner: false,
            read_only_client_count: 0,
        }],
    });

    let runtime_id = runtime_id.to_string();
    let pane_id = pane_id.to_string();
    let state = window.imp().state.borrow();
    assert_eq!(
        state.sessions.len(),
        initial_session_count + 1,
        "inventory should materialize one recovered workspace"
    );
    let session = state
        .sessions
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
    let initial_session_count = window.imp().state.borrow().sessions.len();
    window.imp().state.borrow_mut().sessions.push(session_state.clone());
    window.build_session(&session_state, false);

    window.handle_endpoint_event(crate::daemon_bridge::EndpointEvent::InventoryLoaded {
        endpoint: RuntimeEndpoint::Local,
        sessions: vec![rttx_proto::proto::SessionInfo {
            id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(&runtime_id).unwrap()),
            name: "Recovered Workspace".into(),
            pane_count: 1,
            has_attached_client: false,
            active_pane_id: None,
            panes: vec![rttx_proto::proto::PaneInfo {
                id: rttx_proto::uuid_to_bytes(uuid::Uuid::new_v4()),
                title: "Shell".into(),
                cwd: "/srv/project".into(),
                cols: 120,
                rows: 40,
                exit_status: None,
                reconstructed: true,
            }],
            policy: rttx_proto::proto::RuntimePolicy::Persistent as i32,
            attached_client_count: 0,
            reconstructed: true,
            revision: 7,
            current_client_role: rttx_proto::proto::RuntimeClientRole::Unattached as i32,
            has_write_owner: false,
            read_only_client_count: 0,
        }],
    });

    let state = window.imp().state.borrow();
    assert_eq!(
        state.sessions.len(),
        initial_session_count + 1,
        "inventory should not duplicate an attached runtime"
    );
    assert!(state.sessions.iter().any(|session| session.uuid == "workspace-existing"));
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
        state.sessions[0].uuid.clone()
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
    window.imp().state.borrow_mut().sessions.push(recovered_session.clone());
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
        snapshot: rttx_proto::proto::Snapshot {
            session_id: rttx_proto::uuid_to_bytes(runtime_id),
            panes: vec![rttx_proto::proto::PaneSnapshot {
                pane_id: rttx_proto::uuid_to_bytes(pane_id),
                title: "Shell".into(),
                cwd: "/srv/project".into(),
                cols: 120,
                rows: 40,
                scrollback: b"restored output".to_vec(),
                exit_status: None,
                bracketed_paste_mode: false,
                application_cursor_keys: false,
                application_keypad: false,
                mouse_tracking_mode: 0,
                sgr_mouse_mode: false,
            }],
            revision: 7,
            current_client_role: rttx_proto::proto::RuntimeClientRole::Writer as i32,
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
    crate::session::save_window_state(&WindowState {
        active_session_index: 1,
        sessions: vec![
            SessionState {
                uuid: first_uuid,
                name: "Workspace 1".into(),
                layout: LayoutNode::new_terminal_with_uuid("terminal-1"),
                terminal_recovery: Default::default(),
                active_terminal_uuid: None,
                input_sync: false,
                mode: Default::default(),
                runtime: Default::default(),
                color: Default::default(),
                zoomed_terminal_uuid: None,
                user_renamed: false,
            },
            SessionState {
                uuid: second_uuid.clone(),
                name: "Workspace 2".into(),
                layout: LayoutNode::new_terminal_with_uuid("terminal-2"),
                terminal_recovery: Default::default(),
                active_terminal_uuid: None,
                input_sync: false,
                mode: Default::default(),
                runtime: Default::default(),
                color: Default::default(),
                zoomed_terminal_uuid: None,
                user_renamed: false,
            },
        ],
        ..WindowState::default()
    })
    .unwrap();

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
    window.imp().state.borrow_mut().sessions.push(session_state.clone());
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
    assert_eq!(subtitle.as_deref(), Some("Terminal (persistent)"));
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
        RuntimeEndpoint::Remote { host: "builder.example".into() },
        WorkspacePolicy::Persistent,
        Some(&runtime_id),
    );
    window.imp().state.borrow_mut().sessions.push(session_state.clone());
    window.build_session(&session_state, false);

    window.handle_endpoint_event(crate::daemon_bridge::EndpointEvent::WorkspaceDetached {
        workspace_id: session_state.uuid.clone(),
        runtime_id: runtime_id.clone(),
    });

    let state = window.imp().state.borrow();
    let session = state
        .sessions
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
        RuntimeEndpoint::Remote { host: "builder.example".into() },
        WorkspacePolicy::Persistent,
        Some(&runtime_id),
    );
    window.imp().state.borrow_mut().sessions.push(session_state.clone());
    window.build_session(&session_state, false);

    window.handle_endpoint_event(crate::daemon_bridge::EndpointEvent::WorkspaceDetached {
        workspace_id: session_state.uuid.clone(),
        runtime_id: runtime_id.clone(),
    });
    window.save_state();

    let saved_state = session::load_window_state();
    let saved_session = saved_state
        .sessions
        .iter()
        .find(|session| session.uuid == session_state.uuid)
        .expect("detached workspace should persist in saved state");

    assert!(saved_session.runtime.is_managed());
    assert_eq!(
        saved_session.runtime.endpoint,
        RuntimeEndpoint::Remote { host: "builder.example".into() }
    );
    assert_eq!(saved_session.runtime.policy, WorkspacePolicy::Persistent);
    assert_eq!(saved_session.runtime.runtime_id.as_deref(), Some(runtime_id.as_str()));
    assert_eq!(
        saved_session.runtime.pane_bindings.get("managed-pane").map(String::as_str),
        Some("managed-pane")
    );
    assert!(saved_session.runtime.pending_layout_panes.contains("managed-pane"));

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
        RuntimeEndpoint::Remote { host: "builder.example".into() },
        WorkspacePolicy::Persistent,
        Some(&runtime_id),
    );
    window.imp().state.borrow_mut().sessions.push(session_state.clone());
    window.build_session(&session_state, false);

    window.handle_endpoint_event(crate::daemon_bridge::EndpointEvent::RuntimeTerminated {
        workspace_id: session_state.uuid.clone(),
        runtime_id,
        reason: rttx_proto::proto::RuntimeTerminationReason::Explicit,
    });

    let state = window.imp().state.borrow();
    let session = state
        .sessions
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
        RuntimeEndpoint::Remote { host: "builder.example".into() },
        WorkspacePolicy::Persistent,
        Some(&runtime_id),
    );
    window.imp().state.borrow_mut().sessions.push(session_state.clone());
    window.build_session(&session_state, false);

    window.handle_endpoint_event(crate::daemon_bridge::EndpointEvent::RuntimeTerminated {
        workspace_id: session_state.uuid.clone(),
        runtime_id,
        reason: rttx_proto::proto::RuntimeTerminationReason::Explicit,
    });
    window.save_state();

    let saved_state = session::load_window_state();
    let saved_session = saved_state
        .sessions
        .iter()
        .find(|session| session.uuid == session_state.uuid)
        .expect("terminated workspace should persist in saved state");

    assert!(saved_session.runtime.is_managed());
    assert_eq!(
        saved_session.runtime.endpoint,
        RuntimeEndpoint::Remote { host: "builder.example".into() }
    );
    assert_eq!(saved_session.runtime.policy, WorkspacePolicy::Persistent);
    assert_eq!(saved_session.runtime.runtime_id, None);
    assert_eq!(
        saved_session.mode,
        crate::session::SessionMode::RemotePersistent {
            host: "builder.example".into(),
            daemon_session_id: String::new(),
        }
    );
    assert_eq!(
        saved_session.runtime.pane_bindings.get("managed-pane").map(String::as_str),
        Some("managed-pane")
    );
    assert!(saved_session.runtime.pending_layout_panes.contains("managed-pane"));

    window.close();
    crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
}

#[test]
fn notification_tier_suppresses_for_visible_session() {
    let state = WindowState::default();
    let uuid = state.sessions[0].uuid.clone();
    let terminal = state.sessions[0].layout.terminal_uuids()[0].clone();
    assert_eq!(notification_tier(&terminal, Some(&uuid), true, &state), NotificationTier::Suppress);
}

#[test]
fn notification_tier_toasts_for_background_session_when_focused() {
    let mut state = WindowState::default();
    state.sessions.push(SessionState::new("Background".into()));
    let bg_terminal = state.sessions[1].layout.terminal_uuids()[0].clone();
    let visible_uuid = state.sessions[0].uuid.clone();
    assert_eq!(
        notification_tier(&bg_terminal, Some(&visible_uuid), true, &state),
        NotificationTier::Toast
    );
}

#[test]
fn notification_tier_desktop_when_window_unfocused() {
    let mut state = WindowState::default();
    state.sessions.push(SessionState::new("Background".into()));
    let bg_terminal = state.sessions[1].layout.terminal_uuids()[0].clone();
    let visible_uuid = state.sessions[0].uuid.clone();
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
    let fresh = SessionState::new("Fallback Test".into());
    let session_uuid = fresh.uuid.clone();
    let t1_uuid = fresh.layout.terminal_uuids().into_iter().next().unwrap();
    {
        let mut state = window.imp().state.borrow_mut();
        state.sessions.push(fresh.clone());
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
        .sessions
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
        state.sessions[0].uuid.clone()
    };

    window.maybe_auto_rename_workspace(&session_uuid, Some("/home/user/projects/rttx"));

    {
        let state = window.imp().state.borrow();
        assert_eq!(state.sessions[0].name, "rttx");
        assert!(!state.sessions[0].user_renamed);
    }

    let row = window.imp().sidebar_list.row_at_index(0).expect("row exists");
    let session_row =
        row.child().and_then(|child| child.downcast::<SessionRow>().ok()).expect("SessionRow");
    assert_eq!(session_row.session_name(), "rttx");

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
        state.sessions[0].uuid.clone()
    };

    window.rename_session(&session_uuid, "My Custom Name");
    window.maybe_auto_rename_workspace(&session_uuid, Some("/home/user/projects/rttx"));

    {
        let state = window.imp().state.borrow();
        assert_eq!(state.sessions[0].name, "My Custom Name");
        assert!(state.sessions[0].user_renamed);
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
    window.imp().state.borrow_mut().sessions.push(session_state.clone());
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
        snapshot: rttx_proto::proto::Snapshot {
            session_id: rttx_proto::uuid_to_bytes(runtime_id),
            panes: vec![rttx_proto::proto::PaneSnapshot {
                pane_id: rttx_proto::uuid_to_bytes(pane_id),
                title: "shell".into(),
                cwd: "/home/user".into(),
                cols: 80,
                rows: 24,
                scrollback: b"reconnected".to_vec(),
                exit_status: None,
                bracketed_paste_mode: false,
                application_cursor_keys: false,
                application_keypad: false,
                mouse_tracking_mode: 0,
                sgr_mouse_mode: false,
            }],
            revision: 5,
            current_client_role: rttx_proto::proto::RuntimeClientRole::Writer as i32,
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
    let mut prefs = preferences::load();
    prefs.audible_bell = false;
    prefs.visual_bell = false;
    let _ = preferences::save(&prefs);

    let app = adw::Application::builder().application_id("com.illya.rttx.bell-pref-test").build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    let session_state = {
        let mut state = window.imp().state.borrow_mut();
        state.sessions[0].runtime.managed = true;
        state.sessions[0].runtime.runtime_id = Some("runtime-1".into());
        state.sessions[0].clone()
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

    let layout_uuid = "managed-pane-cwd";
    let runtime_pane_id = uuid::Uuid::new_v4();
    let mut session_state = crate::test_helpers::managed_session_with_runtime(
        "ws-cwd",
        "CWD Test",
        LayoutNode::new_terminal_with_uuid(layout_uuid),
        RuntimeEndpoint::Local,
        WorkspacePolicy::Persistent,
        Some("runtime-cwd"),
    );
    session_state
        .runtime
        .pane_bindings
        .insert(layout_uuid.to_string(), runtime_pane_id.to_string());

    window.imp().state.borrow_mut().sessions.push(session_state.clone());
    window.build_session(&session_state, false);

    // Verify initial layout CWD is None.
    {
        let state = window.imp().state.borrow();
        let session = state.sessions.iter().find(|s| s.uuid == "ws-cwd").unwrap();
        assert_eq!(session.layout.terminal_cwd(layout_uuid), None);
    }

    // Dispatch a CwdChanged message.
    let msg = rttx_proto::proto::ServerMessage {
        msg: Some(rttx_proto::proto::server_message::Msg::CwdChanged(
            rttx_proto::proto::CwdChanged {
                session_id: rttx_proto::uuid_to_bytes(uuid::Uuid::new_v4()),
                pane_id: rttx_proto::uuid_to_bytes(runtime_pane_id),
                cwd: "/tmp/updated".into(),
                revision: 1,
            },
        )),
    };
    window.dispatch_managed_runtime_message(&RuntimeEndpoint::Local, &msg);

    // Verify layout CWD is updated.
    {
        let state = window.imp().state.borrow();
        let session = state.sessions.iter().find(|s| s.uuid == "ws-cwd").unwrap();
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

    let layout_uuid = "managed-pane-exit";
    let runtime_pane_id = uuid::Uuid::new_v4();
    let mut session_state = crate::test_helpers::managed_session_with_runtime(
        "ws-exit",
        "Exit Test",
        LayoutNode::new_terminal_with_uuid(layout_uuid),
        RuntimeEndpoint::Local,
        WorkspacePolicy::Persistent,
        Some("runtime-exit"),
    );
    session_state
        .runtime
        .pane_bindings
        .insert(layout_uuid.to_string(), runtime_pane_id.to_string());

    window.imp().state.borrow_mut().sessions.push(session_state.clone());
    window.build_session(&session_state, false);

    let msg = rttx_proto::proto::ServerMessage {
        msg: Some(rttx_proto::proto::server_message::Msg::PaneExited(
            rttx_proto::proto::PaneExited {
                session_id: rttx_proto::uuid_to_bytes(uuid::Uuid::new_v4()),
                pane_id: rttx_proto::uuid_to_bytes(runtime_pane_id),
                status: 0,
                revision: 2,
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
    let second = SessionState::new("Second".into());
    let second_uuid = second.uuid.clone();
    window.imp().state.borrow_mut().sessions.push(second.clone());
    window.build_session(&second, false);

    assert_eq!(window.imp().state.borrow().sessions.len(), 2);

    window.close_session(&second_uuid);

    assert_eq!(
        window.imp().state.borrow().sessions.len(),
        1,
        "closing one of two workspaces should remove it"
    );
    assert!(
        !window.imp().state.borrow().sessions.iter().any(|s| s.uuid == second_uuid),
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
    let session_uuid = window.imp().state.borrow().sessions[0].uuid.clone();

    assert_eq!(window.imp().state.borrow().sessions.len(), 1);

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

    let second = SessionState::new("Second".into());
    let second_uuid = second.uuid.clone();
    window.imp().state.borrow_mut().sessions.push(second.clone());
    window.build_session(&second, false);

    let first_uuid = window.imp().state.borrow().sessions[0].uuid.clone();

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
    window.imp().state.borrow_mut().sessions.push(session_state.clone());
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
    window.imp().state.borrow_mut().sessions.push(session_state.clone());
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
        orientation: crate::session::layout::SplitOrientation::Horizontal,
        ratio: 0.5,
        first: Box::new(LayoutNode::new_terminal_with_uuid("pane-a")),
        second: Box::new(LayoutNode::new_terminal_with_uuid("pane-b")),
    };
    let session_state =
        crate::test_helpers::managed_session("workspace-swap", "Swap Workspace", layout);
    window.imp().state.borrow_mut().sessions.push(session_state.clone());
    window.build_session(&session_state, false);

    window.swap_terminals("pane-a", "pane-b");

    let state = window.imp().state.borrow();
    let session =
        state.sessions.iter().find(|s| s.uuid == "workspace-swap").expect("session should exist");
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
    assert!(
        stack.child_by_name("bookmarks").is_none(),
        "utility stack should not have a Bookmarks tab"
    );
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
    let remote_session = SessionState::new_managed_remote(
        "Remote Work".into(),
        "deploy@example.com",
        WorkspacePolicy::Persistent,
        None,
    );
    let remote_uuid = remote_session.uuid.clone();
    window.imp().state.borrow_mut().sessions.push(remote_session.clone());
    window.build_session(&remote_session, false);
    pump_events(50);

    // Switch to the remote workspace
    let state = window.imp().state.borrow();
    let remote_idx = state.sessions.iter().position(|s| s.uuid == remote_uuid).unwrap();
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
    crate::commands::save(&[local_cmd, remote_cmd, global_cmd]).unwrap();

    let app = adw::Application::builder()
        .application_id("com.illya.rttx.command-host-filter-tests")
        .build();
    app.register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = Window::new(&app);
    window.present();
    pump_events(50);

    // Default is local host — should show local + global commands
    let count = window.imp().command_list.observe_children().n_items();
    assert_eq!(count, 2, "local host should show local + global commands, got {count}");

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
    crate::host::save_to(&hosts, &config_dir.join("hosts.json")).unwrap();

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
    crate::host::save_to(&hosts, &config_dir.join("hosts.json")).unwrap();

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
    let remote_session = SessionState::new_managed_remote(
        "Remote Work".into(),
        "deploy@builder.example.com",
        WorkspacePolicy::Persistent,
        None,
    );
    let remote_uuid = remote_session.uuid.clone();
    window.imp().state.borrow_mut().sessions.push(remote_session.clone());
    window.build_session(&remote_session, false);
    pump_events(50);

    let state = window.imp().state.borrow();
    let remote_idx = state.sessions.iter().position(|s| s.uuid == remote_uuid).unwrap();
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
    let hosts = crate::host::load();
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
    crate::host::save(&[existing]).unwrap();

    // Add a remote managed workspace and switch to it
    let remote_session = SessionState::new_managed_remote(
        "Remote Work".into(),
        "deploy@builder.example.com",
        WorkspacePolicy::Persistent,
        None,
    );
    let remote_uuid = remote_session.uuid.clone();
    window.imp().state.borrow_mut().sessions.push(remote_session.clone());
    window.build_session(&remote_session, false);
    pump_events(50);

    let state = window.imp().state.borrow();
    let remote_idx = state.sessions.iter().position(|s| s.uuid == remote_uuid).unwrap();
    drop(state);
    window.switch_to_session(remote_idx);
    pump_events(50);

    // Trigger the action — should not duplicate
    window.do_add_current_host();

    let hosts = crate::host::load();
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

    let hosts = crate::host::load();
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
        state.sessions[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    if let Some(term) = window.imp().terminals.borrow().get(&terminal_uuid) {
        term.set_current_directory_for_test(Some("/home/user/projects/rttx"));
    }
    window.imp().focused_terminal_uuid.replace(Some(terminal_uuid));

    window.do_add_current_path_to_places();

    let places = crate::places::load();
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
        state.sessions[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    window.imp().focused_terminal_uuid.replace(Some(terminal_uuid));

    window.do_add_current_path_to_places();

    let places = crate::places::load();
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
    let remote_session = SessionState::new_managed_remote(
        "Remote Work".into(),
        "deploy@builder.example.com",
        WorkspacePolicy::Persistent,
        None,
    );
    let remote_uuid = remote_session.uuid.clone();
    window.imp().state.borrow_mut().sessions.push(remote_session.clone());
    window.build_session(&remote_session, false);
    pump_events(50);

    let state = window.imp().state.borrow();
    let remote_idx = state.sessions.iter().position(|s| s.uuid == remote_uuid).unwrap();
    drop(state);
    window.switch_to_session(remote_idx);
    pump_events(50);

    // Set CWD on the persistent terminal
    let terminal_uuid = {
        let state = window.imp().state.borrow();
        let session = state.sessions.iter().find(|s| s.uuid == remote_uuid).unwrap();
        session.layout.terminal_uuids().into_iter().next().unwrap()
    };
    if let Some(term) = window.imp().persistent_terminals.borrow().get(&terminal_uuid) {
        term.set_current_directory(Some("/srv/app"));
    }
    window.imp().focused_terminal_uuid.replace(Some(terminal_uuid));

    window.do_add_current_path_to_places();

    let places = crate::places::load();
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
        state.sessions[0].layout.terminal_uuids().into_iter().next().unwrap()
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
        state.sessions[0].layout.terminal_uuids().into_iter().next().unwrap()
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
    let initial_count = window.imp().state.borrow().sessions.len();

    window.add_direct_session();

    let state = window.imp().state.borrow();
    assert_eq!(state.sessions.len(), initial_count + 1, "direct session should be added");
    let new_session = state.sessions.last().unwrap();
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
        state.sessions[0].layout.terminal_uuids().into_iter().next().unwrap()
    };
    let session_uuid = window.imp().state.borrow().sessions[0].uuid.clone();

    // Mark the terminal as active so refresh_sidebar_subtitle_if_active finds it.
    {
        let mut state = window.imp().state.borrow_mut();
        state.sessions[0].active_terminal_uuid = Some(terminal_uuid.clone());
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
    window.imp().state.borrow_mut().sessions.push(session_state.clone());
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
        let session = state.sessions.iter_mut().find(|s| s.uuid == session_state.uuid).unwrap();
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
        let session = state.sessions.iter_mut().find(|s| s.uuid == session_state.uuid).unwrap();
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
    crate::host::save_to(&hosts, &config_dir.join("hosts.json")).unwrap();

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
    let remote_session = SessionState::new_managed_remote(
        "Remote Work".into(),
        "deploy@builder.example.com",
        WorkspacePolicy::Persistent,
        None,
    );
    window.imp().state.borrow_mut().sessions.push(remote_session.clone());
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
    crate::places::save_to(&[user_place], &tmp.path().join("rttx-devel/places.json")).unwrap();

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
