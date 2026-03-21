use gtk4::glib;
use gtk4::glib::subclass::prelude::*;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;
use vte4::prelude::*;

use crate::color_scheme;
use crate::config;
use crate::preferences;
use crate::session::{self, LayoutNode, SessionState, SplitOrientation, WindowState};
use crate::sidebar::SessionRow;
use crate::terminal::widget::TerminalWidget;

mod imp {
    use super::*;
    use std::cell::RefCell;

    pub struct Window {
        pub split_view: adw::OverlaySplitView,
        pub sidebar_list: gtk4::ListBox,
        pub session_stack: gtk4::Stack,
        pub add_session_button: gtk4::Button,
        pub state: RefCell<WindowState>,
        pub terminals: RefCell<std::collections::HashMap<String, TerminalWidget>>,
        pub focused_terminal_uuid: RefCell<Option<String>>,
    }

    impl Default for Window {
        fn default() -> Self {
            Self {
                split_view: adw::OverlaySplitView::new(),
                sidebar_list: gtk4::ListBox::new(),
                session_stack: gtk4::Stack::new(),
                add_session_button: gtk4::Button::from_icon_name("list-add-symbolic"),
                state: RefCell::new(WindowState::default()),
                terminals: RefCell::new(std::collections::HashMap::new()),
                focused_terminal_uuid: RefCell::new(None),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Window {
        const NAME: &'static str = "RttxWindow";
        type Type = super::Window;
        type ParentType = adw::ApplicationWindow;
    }

    impl ObjectImpl for Window {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            obj.set_default_size(900, 600);
            obj.set_title(Some("rttx"));

            // Header bar
            let header = adw::HeaderBar::new();
            let toggle_sidebar = gtk4::ToggleButton::new();
            toggle_sidebar.set_icon_name("sidebar-show-symbolic");
            toggle_sidebar.set_tooltip_text(Some("Toggle sidebar"));
            toggle_sidebar.set_active(true);
            header.pack_start(&toggle_sidebar);

            self.add_session_button
                .set_tooltip_text(Some("New session"));
            header.pack_start(&self.add_session_button);

            let menu_button = gtk4::MenuButton::new();
            menu_button.set_icon_name("open-menu-symbolic");

            let menu = gtk4::gio::Menu::new();
            menu.append(Some("Preferences"), Some("win.preferences"));
            menu.append(Some("Sync Input"), Some("win.toggle-input-sync"));
            menu.append(Some("Keyboard Shortcuts"), Some("win.show-help-overlay"));
            menu.append(Some("Fullscreen"), Some("win.fullscreen"));
            menu_button.set_menu_model(Some(&menu));

            header.pack_end(&menu_button);

            // Sidebar
            self.sidebar_list
                .set_selection_mode(gtk4::SelectionMode::Single);
            self.sidebar_list.add_css_class("navigation-sidebar");
            self.sidebar_list
                .update_property(&[gtk4::accessible::Property::Label("Sessions")]);

            let sidebar_scroll = gtk4::ScrolledWindow::builder()
                .hscrollbar_policy(gtk4::PolicyType::Never)
                .vexpand(true)
                .width_request(200)
                .child(&self.sidebar_list)
                .build();

            // Session content area
            self.session_stack.set_hexpand(true);
            self.session_stack.set_vexpand(true);

            // OverlaySplitView: sidebar | content
            self.split_view.set_sidebar(Some(&sidebar_scroll));
            self.split_view.set_content(Some(&self.session_stack));
            self.split_view.set_show_sidebar(true);
            self.split_view.set_collapsed(false);
            self.split_view.set_min_sidebar_width(180.0);
            self.split_view.set_max_sidebar_width(300.0);

            // Bind toggle button to split view
            self.split_view
                .bind_property("show-sidebar", &toggle_sidebar, "active")
                .bidirectional()
                .sync_create()
                .build();

            // Main layout
            let main_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            main_box.append(&header);
            main_box.append(&self.split_view);

            obj.set_content(Some(&main_box));
        }
    }

    impl WidgetImpl for Window {}
    impl WindowImpl for Window {}
    impl ApplicationWindowImpl for Window {}
    impl AdwApplicationWindowImpl for Window {}
}

glib::wrapper! {
    pub struct Window(ObjectSubclass<imp::Window>)
        @extends adw::ApplicationWindow, gtk4::ApplicationWindow, gtk4::Window, gtk4::Widget,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget,
                    gtk4::Native, gtk4::Root, gtk4::ShortcutManager,
                    gtk4::gio::ActionGroup, gtk4::gio::ActionMap;
}

impl Window {
    pub fn new(app: &adw::Application) -> Self {
        let obj: Self = glib::Object::builder().property("application", app).build();
        obj.setup_actions(app);
        obj.setup_signals();
        // Register the shortcuts overlay so the "Keyboard Shortcuts" menu item works.
        // set_help_overlay automatically provides the win.show-help-overlay action.
        obj.set_help_overlay(Some(&Self::build_shortcuts_window()));
        obj.restore_state();
        obj
    }

    fn build_shortcuts_window() -> gtk4::ShortcutsWindow {
        fn sc(title: &str, accel: &str) -> gtk4::ShortcutsShortcut {
            gtk4::ShortcutsShortcut::builder()
                .title(title)
                .accelerator(accel)
                .build()
        }

        let sessions = gtk4::ShortcutsGroup::builder().title("Sessions").build();
        sessions.append(&sc("New Session", "<Ctrl><Shift>t"));
        sessions.append(&sc("Close Terminal", "<Ctrl><Shift>w"));
        sessions.append(&sc("Next Session", "<Ctrl>Tab"));
        sessions.append(&sc("Previous Session", "<Ctrl><Shift>Tab"));
        sessions.append(&sc("Toggle Sidebar", "<Ctrl><Shift>n"));

        let splits = gtk4::ShortcutsGroup::builder().title("Splits").build();
        splits.append(&sc("Split Right", "<Ctrl><Shift>e"));
        splits.append(&sc("Split Down", "<Ctrl><Shift>o"));

        let terminal = gtk4::ShortcutsGroup::builder().title("Terminal").build();
        terminal.append(&sc("Copy", "<Ctrl><Shift>c"));
        terminal.append(&sc("Paste", "<Ctrl><Shift>v"));
        terminal.append(&sc("Find", "<Ctrl><Shift>f"));
        terminal.append(&sc("Zoom In", "<Ctrl>plus"));
        terminal.append(&sc("Zoom Out", "<Ctrl>minus"));
        terminal.append(&sc("Reset Zoom", "<Ctrl>0"));

        let app_group = gtk4::ShortcutsGroup::builder().title("Application").build();
        app_group.append(&sc("Preferences", "<Ctrl>comma"));
        app_group.append(&sc("Toggle Input Sync", "<Ctrl><Shift>i"));
        app_group.append(&sc("Fullscreen", "F11"));

        let section = gtk4::ShortcutsSection::builder()
            .section_name("shortcuts")
            .build();
        section.append(&sessions);
        section.append(&splits);
        section.append(&terminal);
        section.append(&app_group);

        let win = gtk4::ShortcutsWindow::builder().modal(true).build();
        win.add_section(&section);
        win
    }

    fn setup_actions(&self, app: &adw::Application) {
        let actions: &[(&str, &[&str], fn(&Window))] = &[
            ("new-session", &["<Ctrl><Shift>t"], |w| w.add_session()),
            ("close-terminal", &["<Ctrl><Shift>w"], |w| {
                w.close_focused_terminal()
            }),
            ("split-h", &["<Ctrl><Shift>e"], |w| {
                w.split_focused(SplitOrientation::Horizontal)
            }),
            ("split-v", &["<Ctrl><Shift>o"], |w| {
                w.split_focused(SplitOrientation::Vertical)
            }),
            ("toggle-search", &["<Ctrl><Shift>f"], |w| {
                w.toggle_focused_search()
            }),
            ("toggle-sidebar", &["<Ctrl><Shift>n"], |w| {
                let sv = &w.imp().split_view;
                sv.set_show_sidebar(!sv.shows_sidebar());
            }),
            ("fullscreen", &["F11"], |w| {
                if w.is_fullscreen() {
                    w.unfullscreen()
                } else {
                    w.fullscreen()
                }
            }),
            ("next-session", &["<Ctrl>Tab"], |w| w.cycle_session(1)),
            ("prev-session", &["<Ctrl><Shift>Tab"], |w| {
                w.cycle_session(-1)
            }),
            ("zoom-in", &["<Ctrl>plus", "<Ctrl>equal"], |w| {
                w.zoom_focused(1)
            }),
            ("zoom-out", &["<Ctrl>minus"], |w| w.zoom_focused(-1)),
            ("zoom-reset", &["<Ctrl>0"], |w| w.zoom_focused(0)),
            ("copy", &["<Ctrl><Shift>c"], |w| w.clipboard_copy()),
            ("paste", &["<Ctrl><Shift>v"], |w| w.clipboard_paste()),
        ];

        for &(name, accels, callback) in actions {
            let action = gtk4::gio::SimpleAction::new(name, None);
            let win = self.clone();
            action.connect_activate(move |_, _| callback(&win));
            self.add_action(&action);
            app.set_accels_for_action(&format!("win.{name}"), accels);
        }

        // Input sync toggle (stateful action)
        let sync_action =
            gtk4::gio::SimpleAction::new_stateful("toggle-input-sync", None, &false.to_variant());
        let win = self.clone();
        sync_action.connect_activate(move |action, _| {
            let current = action
                .state()
                .and_then(|v| v.get::<bool>())
                .unwrap_or(false);
            let new_val = !current;
            action.set_state(&new_val.to_variant());
            win.set_input_sync(new_val);
        });
        self.add_action(&sync_action);
        app.set_accels_for_action("win.toggle-input-sync", &["<Ctrl><Shift>i"]);

        // Preferences action (no accelerator, triggered from menu)
        let prefs_action = gtk4::gio::SimpleAction::new("preferences", None);
        let win = self.clone();
        prefs_action.connect_activate(move |_, _| {
            crate::preferences_window::show(&win);
        });
        self.add_action(&prefs_action);
        app.set_accels_for_action("win.preferences", &["<Ctrl>comma"]);
    }

    fn setup_signals(&self) {
        let win = self.clone();
        self.imp().add_session_button.connect_clicked(move |_| {
            win.add_session();
        });

        let win = self.clone();
        self.imp().sidebar_list.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                let index = row.index() as usize;
                win.switch_to_session(index);
            }
        });

        let win = self.clone();
        self.connect_close_request(move |_| {
            win.save_state();
            glib::Propagation::Proceed
        });
    }

    fn restore_state(&self) {
        let state = session::load_window_state();

        if state.is_maximized {
            self.maximize();
        } else {
            self.set_default_size(state.width, state.height);
        }

        for session_state in &state.sessions {
            self.build_session(session_state);
        }

        *self.imp().state.borrow_mut() = state.clone();

        if let Some(row) = self
            .imp()
            .sidebar_list
            .row_at_index(state.active_session_index as i32)
        {
            self.imp().sidebar_list.select_row(Some(&row));
        }
    }

    fn save_state(&self) {
        let state = self.capture_state();
        if let Err(e) = session::save_window_state(&state) {
            log::error!("Failed to save window state: {}", e);
        }
    }

    fn capture_state(&self) -> WindowState {
        let imp = self.imp();
        let mut sessions = Vec::new();

        let state = imp.state.borrow();
        for session_state in &state.sessions {
            let mut captured = session_state.clone();
            self.update_cwds(&mut captured.layout);
            // Capture current Paned divider positions as ratios so they are
            // restored correctly on the next launch.
            if let Some(root) = imp.session_stack.child_by_name(&session_state.uuid) {
                session::capture_paned_ratios(&mut captured.layout, &root);
            }
            sessions.push(captured);
        }

        let active_index = imp
            .sidebar_list
            .selected_row()
            .map(|r| r.index() as usize)
            .unwrap_or(0);

        let (width, height) = self.default_size();

        WindowState {
            sessions,
            active_session_index: active_index,
            width: width.max(1),
            height: height.max(1),
            is_maximized: self.is_maximized(),
        }
    }

    fn update_cwds(&self, layout: &mut LayoutNode) {
        match layout {
            LayoutNode::Terminal {
                uuid,
                cwd,
                custom_title,
                ..
            } => {
                if let Some(term) = self.imp().terminals.borrow().get(uuid.as_str()) {
                    *cwd = term.current_directory();
                    *custom_title = term.custom_title();
                }
            }
            LayoutNode::Split { first, second, .. } => {
                self.update_cwds(first);
                self.update_cwds(second);
            }
        }
    }

    fn build_session(&self, session_state: &SessionState) {
        let imp = self.imp();

        let win = self.clone();
        let content =
            session::build_layout_widget(&session_state.layout, &|uuid, cwd, _, custom_title| {
                let term = TerminalWidget::new(uuid, cwd);
                if let Some(title) = custom_title {
                    term.set_title(title);
                    term.imp().custom_title.replace(Some(title.to_string()));
                }
                win.connect_terminal_signals(&term);
                win.imp()
                    .terminals
                    .borrow_mut()
                    .insert(uuid.to_string(), term.clone());
                term.upcast()
            });

        imp.session_stack
            .add_named(&content, Some(&session_state.uuid));
        Self::schedule_apply_paned_ratios(&content, &session_state.layout);

        let row = SessionRow::new(
            &session_state.uuid,
            &session_state.name,
            session_state.layout.terminal_count(),
        );

        let win = self.clone();
        let session_uuid = session_state.uuid.clone();
        row.close_button().connect_clicked(move |_| {
            win.close_session(&session_uuid);
        });

        let list_row = gtk4::ListBoxRow::new();
        list_row.set_child(Some(&row));
        imp.sidebar_list.append(&list_row);
    }

    fn connect_terminal_signals(&self, term: &TerminalWidget) {
        // Apply current preferences
        self.apply_preferences_to_terminal(term);

        // Track focus
        let win = self.clone();
        let uuid = term.uuid();
        let focus_controller = gtk4::EventControllerFocus::new();
        focus_controller.connect_enter(move |_| {
            win.imp().focused_terminal_uuid.replace(Some(uuid.clone()));
        });
        term.vte().add_controller(focus_controller);

        // Input sync: forward commit text to sibling terminals
        let win = self.clone();
        let uuid = term.uuid();
        term.vte().connect_commit(move |_, text, _| {
            win.forward_input(&uuid, text);
        });

        // Drag and drop: drag from header to swap terminals
        let drag_source = gtk4::DragSource::new();
        drag_source.set_actions(gtk4::gdk::DragAction::MOVE);
        let uuid = term.uuid();
        drag_source.connect_prepare(move |_, _, _| {
            Some(gtk4::gdk::ContentProvider::for_value(&uuid.to_value()))
        });
        term.imp().header.add_controller(drag_source);

        let drop_target = gtk4::DropTarget::new(glib::Type::STRING, gtk4::gdk::DragAction::MOVE);
        let win = self.clone();
        let target_uuid = term.uuid();
        drop_target.connect_drop(move |_, value, _, _| {
            if let Ok(source_uuid) = value.get::<String>() {
                if source_uuid != target_uuid {
                    win.swap_terminals(&source_uuid, &target_uuid);
                    return true;
                }
            }
            false
        });
        term.add_controller(drop_target);

        let win = self.clone();
        let uuid = term.uuid();
        term.split_h_button().connect_clicked(move |_| {
            win.split_terminal(&uuid, SplitOrientation::Horizontal);
        });

        let win = self.clone();
        let uuid = term.uuid();
        term.split_v_button().connect_clicked(move |_| {
            win.split_terminal(&uuid, SplitOrientation::Vertical);
        });

        let win = self.clone();
        let uuid = term.uuid();
        term.close_button().connect_clicked(move |_| {
            win.close_terminal(&uuid);
        });

        let win = self.clone();
        let uuid = term.uuid();
        let handler_id = term.vte().connect_child_exited(move |_, status| {
            // Notify if this terminal wasn't focused
            let focused = win.imp().focused_terminal_uuid.borrow().clone();
            if focused.as_deref() != Some(&uuid) {
                win.notify_process_completed(&uuid, status);
            }
            win.close_terminal(&uuid);
        });
        term.imp().child_exited_handler.replace(Some(handler_id));
    }

    pub fn add_session(&self) {
        let imp = self.imp();
        let count = imp.state.borrow().sessions.len() + 1;
        let session_state = SessionState::new(format!("Session {}", count));
        imp.state.borrow_mut().sessions.push(session_state.clone());
        self.build_session(&session_state);

        let index = imp.state.borrow().sessions.len() as i32 - 1;
        if let Some(row) = imp.sidebar_list.row_at_index(index) {
            imp.sidebar_list.select_row(Some(&row));
        }
    }

    fn switch_to_session(&self, index: usize) {
        let imp = self.imp();
        let (uuid, input_sync) = {
            let state = imp.state.borrow();
            let Some(session) = state.sessions.get(index) else {
                return;
            };
            (session.uuid.clone(), session.input_sync)
        };
        imp.session_stack.set_visible_child_name(&uuid);
        // Keep the toggle-input-sync menu button in sync with the session we
        // just switched to.  Without this update the button can show the
        // previous session's state.
        if let Some(action) = self.lookup_action("toggle-input-sync") {
            if let Ok(action) = action.downcast::<gtk4::gio::SimpleAction>() {
                action.set_state(&input_sync.to_variant());
            }
        }
    }

    fn close_session(&self, session_uuid: &str) {
        let imp = self.imp();

        // Step 1: Extract what we need from state, then release the borrow.
        let (terminal_uuids, new_index) = {
            let mut state = imp.state.borrow_mut();
            if state.sessions.len() <= 1 {
                return;
            }
            let Some(pos) = state.sessions.iter().position(|s| s.uuid == session_uuid) else {
                return;
            };
            let session = state.sessions.remove(pos);
            let uuids = session.layout.terminal_uuids();
            let new_index = pos.min(state.sessions.len() - 1);
            state.active_session_index = new_index;
            (uuids, new_index)
        };
        // state borrow is released here — safe to do widget ops.

        // Step 2: Disconnect child_exited handlers BEFORE removing widgets,
        // so VTE dropping doesn't fire signals back into our borrowed state.
        {
            let terminals = imp.terminals.borrow();
            for uuid in &terminal_uuids {
                if let Some(term) = terminals.get(uuid) {
                    term.disconnect_child_exited();
                }
            }
        }

        // Step 3: Remove terminals from our map.
        {
            let mut terminals = imp.terminals.borrow_mut();
            for uuid in &terminal_uuids {
                terminals.remove(uuid);
            }
        }

        // Step 4: Remove widgets from the UI.
        if let Some(child) = imp.session_stack.child_by_name(session_uuid) {
            imp.session_stack.remove(&child);
        }
        if let Some(row) = imp.sidebar_list.row_at_index(
            // We already removed from state, so find the row by iterating
            // (the row index may not match pos if rows were reordered).
            {
                let mut idx = 0;
                loop {
                    match imp.sidebar_list.row_at_index(idx) {
                        Some(r) => {
                            if let Some(sr) =
                                r.child().and_then(|c| c.downcast::<SessionRow>().ok())
                            {
                                if sr.uuid() == session_uuid {
                                    break idx;
                                }
                            }
                            idx += 1;
                        }
                        None => break -1,
                    }
                }
            },
        ) {
            imp.sidebar_list.remove(&row);
        }

        if let Some(row) = imp.sidebar_list.row_at_index(new_index as i32) {
            imp.sidebar_list.select_row(Some(&row));
        }
    }

    fn split_terminal(&self, terminal_uuid: &str, orientation: SplitOrientation) {
        let imp = self.imp();
        let mut state = imp.state.borrow_mut();

        let session_idx = state.sessions.iter().position(|s| {
            s.layout
                .terminal_uuids()
                .contains(&terminal_uuid.to_string())
        });

        if let Some(idx) = session_idx {
            if let Some((new_layout, new_terminal_uuid)) = state.sessions[idx]
                .layout
                .split_terminal_with_new_uuid(terminal_uuid, orientation)
            {
                state.sessions[idx].layout = new_layout;
                let session_uuid = state.sessions[idx].uuid.clone();
                let session_state = state.sessions[idx].clone();
                drop(state);
                if !self.split_terminal_in_place(
                    &session_uuid,
                    terminal_uuid,
                    &new_terminal_uuid,
                    orientation,
                ) {
                    self.rebuild_session_content(&session_uuid, &session_state);
                } else {
                    self.update_sidebar_count(
                        &session_uuid,
                        session_state.layout.terminal_count(),
                    );
                }
            }
        }
    }

    fn close_terminal(&self, terminal_uuid: &str) {
        let imp = self.imp();

        // Step 1: Update state, extract what we need, release borrow.
        enum Action {
            CloseSession(String),
            Rebuild {
                session_uuid: String,
                session_state: SessionState,
            },
        }

        let action = {
            let mut state = imp.state.borrow_mut();
            let session_idx = state.sessions.iter().position(|s| {
                s.layout
                    .terminal_uuids()
                    .contains(&terminal_uuid.to_string())
            });
            let Some(idx) = session_idx else { return };

            if state.sessions[idx].layout.terminal_count() <= 1 {
                Action::CloseSession(state.sessions[idx].uuid.clone())
            } else if let Some(new_layout) =
                state.sessions[idx].layout.remove_terminal(terminal_uuid)
            {
                state.sessions[idx].layout = new_layout;
                Action::Rebuild {
                    session_uuid: state.sessions[idx].uuid.clone(),
                    session_state: state.sessions[idx].clone(),
                }
            } else {
                return;
            }
        };
        // state borrow released.

        match action {
            Action::CloseSession(uuid) => self.close_session(&uuid),
            Action::Rebuild {
                session_uuid,
                session_state,
            } => {
                // Disconnect signal before removing from map
                if let Some(term) = imp.terminals.borrow().get(terminal_uuid) {
                    term.disconnect_child_exited();
                }
                imp.terminals.borrow_mut().remove(terminal_uuid);
                self.rebuild_session_content(&session_uuid, &session_state);
            }
        }
    }

    /// Rebuild the widget tree for a session.
    ///
    /// Key insight: we must unparent all existing TerminalWidgets from the
    /// old Paned tree BEFORE building the new tree. GTK4 does not allow a
    /// widget to be added to a new parent while still attached to an old one.
    /// We also must NOT reconnect signals on reused terminals — they already
    /// have their handlers from the initial build_session call.
    fn rebuild_session_content(&self, session_uuid: &str, session_state: &SessionState) {
        let imp = self.imp();

        // Step 1: Remove old container from the stack FIRST.
        // This detaches the entire widget subtree (Paned + terminals) from
        // the stack. We must do this before unparenting terminals, because
        // if the session has a single terminal, that terminal IS the stack's
        // direct child — unparenting it first would break the stack's
        // parent-child invariant.
        let old_content = imp.session_stack.child_by_name(session_uuid);
        if let Some(ref old) = old_content {
            imp.session_stack.remove(old);
        }

        // Step 2: Unparent all existing terminals from the now-detached
        // Paned tree so they can be reparented into the new tree.
        //
        // Important: detach via the old container API (`set_*_child(None)`),
        // not via `child.unparent()`. Calling `unparent()` directly on a
        // reused leaf leaves stale child pointers behind in the detached old
        // `GtkPaned`s, and when that old tree is later destroyed it clears the
        // child's parent pointer out from under the new tree. The result is
        // exactly the observed bug: after multiple splits only the newest
        // terminal remains live.
        if let Some(ref old) = old_content {
            Self::detach_terminals_from_detached_tree(old);
        }
        // Drop the old content reference so the detached Paned tree can be freed.
        drop(old_content);

        // Step 3: Build new widget tree, reusing existing terminals and only
        // creating + connecting signals for genuinely new ones.
        let win = self.clone();
        let content =
            session::build_layout_widget(&session_state.layout, &|uuid, cwd, _, custom_title| {
                // Reuse existing terminal (already has signal handlers)
                let existing = {
                    let terminals = win.imp().terminals.borrow();
                    terminals.get(uuid).cloned()
                };
                if let Some(existing) = existing {
                    // Defensive detach: if a reused terminal is still attached to
                    // the detached old tree, GTK will refuse to insert it into the
                    // new Paned hierarchy and the leaf will render blank.
                    if existing.parent().is_some() {
                        existing.unparent();
                    }
                    return existing.upcast();
                }

                // New terminal — create and connect signals
                let term = TerminalWidget::new(uuid, cwd);
                if let Some(title) = custom_title {
                    term.set_title(title);
                    term.imp().custom_title.replace(Some(title.to_string()));
                }
                win.connect_terminal_signals(&term);
                win.imp()
                    .terminals
                    .borrow_mut()
                    .insert(uuid.to_string(), term.clone());
                term.upcast()
            });

        imp.session_stack.add_named(&content, Some(session_uuid));
        imp.session_stack.set_visible_child_name(session_uuid);

        Self::schedule_apply_paned_ratios(&content, &session_state.layout);

        self.update_sidebar_count(session_uuid, session_state.layout.terminal_count());
    }

    fn detach_terminals_from_detached_tree(widget: &gtk4::Widget) {
        if let Some(paned) = widget.downcast_ref::<gtk4::Paned>() {
            if let Some(start) = paned.start_child() {
                Self::detach_terminals_from_detached_tree(&start);
                paned.set_start_child(None::<&gtk4::Widget>);
            }
            if let Some(end) = paned.end_child() {
                Self::detach_terminals_from_detached_tree(&end);
                paned.set_end_child(None::<&gtk4::Widget>);
            }
        }
    }

    fn schedule_apply_paned_ratios(content: &gtk4::Widget, layout: &LayoutNode) {
        let LayoutNode::Split { orientation, .. } = layout else {
            return;
        };

        // Fast path: one idle turn is often enough once the new tree has been
        // attached to the stack and GTK has finished the immediate layout work.
        let idle_layout = layout.clone();
        glib::idle_add_local_once(glib::clone!(
            #[weak]
            content,
            move || {
                session::apply_paned_ratios(&idle_layout, &content);
            }
        ));

        // The idle pass is not sufficient in every live rebuild. In practice
        // the root Paned can still be sitting at its fallback 200px divider
        // when the idle runs. Apply again on the first rendered frame whose
        // root Paned has a real allocation, then stop so user drags are not
        // overridden on subsequent frames.
        let tick_layout = layout.clone();
        let root_orientation = *orientation;
        content.add_tick_callback(move |widget, _| {
            session::apply_paned_ratios(&tick_layout, widget);

            let Some(paned) = widget.downcast_ref::<gtk4::Paned>() else {
                return glib::ControlFlow::Break;
            };
            let total = match root_orientation {
                SplitOrientation::Horizontal => paned.width(),
                SplitOrientation::Vertical => paned.height(),
            };
            if total > 0 {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    fn split_terminal_in_place(
        &self,
        session_uuid: &str,
        target_uuid: &str,
        new_terminal_uuid: &str,
        orientation: SplitOrientation,
    ) -> bool {
        let imp = self.imp();
        let target = {
            let terminals = imp.terminals.borrow();
            terminals.get(target_uuid).cloned()
        };
        let Some(target) = target else {
            return false;
        };

        let parent = target.parent();
        let Some(parent) = parent else {
            return false;
        };

        let new_term = TerminalWidget::new(new_terminal_uuid, None);
        self.connect_terminal_signals(&new_term);
        imp.terminals
            .borrow_mut()
            .insert(new_terminal_uuid.to_string(), new_term.clone());

        let branch_layout = LayoutNode::Split {
            orientation,
            ratio: 0.5,
            first: Box::new(LayoutNode::Terminal {
                uuid: target_uuid.to_string(),
                profile: None,
                cwd: None,
                custom_title: None,
            }),
            second: Box::new(LayoutNode::Terminal {
                uuid: new_terminal_uuid.to_string(),
                profile: None,
                cwd: None,
                custom_title: None,
            }),
        };

        let build_branch = || {
            session::build_layout_widget(&branch_layout, &|uuid, _, _, _| {
                if uuid == target_uuid {
                    target.clone().upcast()
                } else if uuid == new_terminal_uuid {
                    new_term.clone().upcast()
                } else {
                    unreachable!("split branch builder requested unexpected uuid {uuid}");
                }
            })
        };

        if let Ok(stack) = parent.clone().downcast::<gtk4::Stack>() {
            stack.remove(&target);
            let branch = build_branch();
            stack.add_named(&branch, Some(session_uuid));
            stack.set_visible_child_name(session_uuid);
            Self::schedule_apply_paned_ratios(&branch, &branch_layout);
            return true;
        }

        let Ok(paned) = parent.downcast::<gtk4::Paned>() else {
            imp.terminals.borrow_mut().remove(new_terminal_uuid);
            return false;
        };

        let target_widget = target.clone().upcast::<gtk4::Widget>();
        let start_child = paned.start_child();
        let end_child = paned.end_child();
        let is_start = start_child.as_ref() == Some(&target_widget);
        let is_end = end_child.as_ref() == Some(&target_widget);

        if !is_start && !is_end {
            imp.terminals.borrow_mut().remove(new_terminal_uuid);
            return false;
        }

        if is_start {
            paned.set_start_child(None::<&gtk4::Widget>);
        } else {
            paned.set_end_child(None::<&gtk4::Widget>);
        }

        let branch = build_branch();
        if is_start {
            paned.set_start_child(Some(&branch));
        } else {
            paned.set_end_child(Some(&branch));
        }
        Self::schedule_apply_paned_ratios(&branch, &branch_layout);
        true
    }

    fn update_sidebar_count(&self, session_uuid: &str, count: usize) {
        let imp = self.imp();
        let mut idx = 0;
        while let Some(row) = imp.sidebar_list.row_at_index(idx) {
            if let Some(session_row) = row.child().and_then(|c| c.downcast::<SessionRow>().ok()) {
                if session_row.uuid() == session_uuid {
                    session_row.update_terminal_count(count);
                    return;
                }
            }
            idx += 1;
        }
    }

    // ── Keyboard shortcut helpers ────────────────────────────────────

    fn set_input_sync(&self, enabled: bool) {
        let mut state = self.imp().state.borrow_mut();
        let active_idx = self
            .imp()
            .sidebar_list
            .selected_row()
            .map(|r| r.index() as usize)
            .unwrap_or(0);
        if let Some(session) = state.sessions.get_mut(active_idx) {
            session.input_sync = enabled;
        }
    }

    /// Apply all user preferences (font, colors, scrollback, bell, …) to a
    /// terminal widget.  Called when a new terminal is created and when
    /// preferences change.
    fn apply_preferences_to_terminal(&self, term: &TerminalWidget) {
        let prefs = preferences::load();
        let vte = term.vte();
        let font_desc = gtk4::pango::FontDescription::from_string(&prefs.font);
        vte.set_font(Some(&font_desc));
        vte.set_scrollback_lines(prefs.scrollback_lines);
        vte.set_scroll_on_keystroke(prefs.scroll_on_keystroke);
        vte.set_scroll_on_output(prefs.scroll_on_output);
        vte.set_audible_bell(prefs.audible_bell);

        // Header visibility
        if !prefs.show_headerbar {
            term.imp().header.set_visible(false);
        }

        // Load and apply color scheme
        if prefs.color_scheme != "default" {
            let mut scheme_path = glib::user_config_dir();
            scheme_path.push(config::CONFIG_DIR);
            scheme_path.push(config::SCHEMES_DIR);
            scheme_path.push(format!("{}.json", prefs.color_scheme));
            if let Ok(scheme) = color_scheme::load_scheme_file(&scheme_path) {
                term.apply_color_scheme(&scheme);
            }
        }
    }

    /// Forward input from one terminal to all siblings in the same session
    /// when input sync is enabled.
    fn forward_input(&self, source_uuid: &str, text: &str) {
        let state = self.imp().state.borrow();
        let session = state
            .sessions
            .iter()
            .find(|s| s.input_sync && s.layout.terminal_uuids().contains(&source_uuid.to_string()));
        let Some(session) = session else { return };
        let uuids = session.layout.terminal_uuids();
        drop(state);

        let terminals = self.imp().terminals.borrow();
        for uuid in &uuids {
            if uuid != source_uuid {
                if let Some(term) = terminals.get(uuid) {
                    term.vte().feed_child(text.as_bytes());
                }
            }
        }
    }

    fn focused_terminal_uuid(&self) -> Option<String> {
        self.imp().focused_terminal_uuid.borrow().clone()
    }

    fn close_focused_terminal(&self) {
        if let Some(uuid) = self.focused_terminal_uuid() {
            self.close_terminal(&uuid);
        }
    }

    fn split_focused(&self, orientation: SplitOrientation) {
        if let Some(uuid) = self.focused_terminal_uuid() {
            self.split_terminal(&uuid, orientation);
        }
    }

    fn toggle_focused_search(&self) {
        if let Some(uuid) = self.focused_terminal_uuid() {
            if let Some(term) = self.imp().terminals.borrow().get(&uuid) {
                term.toggle_search();
            }
        }
    }

    fn cycle_session(&self, delta: i32) {
        let imp = self.imp();
        let state = imp.state.borrow();
        let len = state.sessions.len() as i32;
        if len == 0 {
            return;
        }
        let current = imp
            .sidebar_list
            .selected_row()
            .map(|r| r.index())
            .unwrap_or(0);
        let next = (current + delta).rem_euclid(len);
        drop(state);
        if let Some(row) = imp.sidebar_list.row_at_index(next) {
            imp.sidebar_list.select_row(Some(&row));
        }
    }

    fn swap_terminals(&self, uuid_a: &str, uuid_b: &str) {
        let imp = self.imp();
        let (session_uuid, session_state) = {
            let mut state = imp.state.borrow_mut();
            let session = state
                .sessions
                .iter_mut()
                .find(|s| s.layout.contains_terminal(uuid_a) && s.layout.contains_terminal(uuid_b));
            let Some(session) = session else { return };
            session.layout.swap_terminals(uuid_a, uuid_b);
            (session.uuid.clone(), session.clone())
        };
        self.rebuild_session_content(&session_uuid, &session_state);
    }

    fn zoom_focused(&self, direction: i32) {
        if let Some(uuid) = self.focused_terminal_uuid() {
            if let Some(term) = self.imp().terminals.borrow().get(&uuid) {
                let vte = term.vte();
                match direction {
                    1 => {
                        let s = vte.font_scale();
                        vte.set_font_scale(s * 1.1);
                    }
                    -1 => {
                        let s = vte.font_scale();
                        vte.set_font_scale(s / 1.1);
                    }
                    _ => vte.set_font_scale(1.0),
                }
            }
        }
    }

    fn notify_process_completed(&self, terminal_uuid: &str, status: i32) {
        let title = self
            .imp()
            .terminals
            .borrow()
            .get(terminal_uuid)
            .map(|t| t.title_label().label().to_string())
            .unwrap_or_else(|| "Terminal".into());

        let body = if status == 0 {
            format!("\"{}\" completed successfully", title)
        } else {
            format!("\"{}\" exited with status {}", title, status)
        };

        let notification = gtk4::gio::Notification::new("Process completed");
        notification.set_body(Some(&body));
        if let Some(app) = self.application() {
            app.send_notification(None, &notification);
        }
    }

    fn clipboard_copy(&self) {
        if let Some(uuid) = self.focused_terminal_uuid() {
            if let Some(term) = self.imp().terminals.borrow().get(&uuid) {
                term.vte().copy_clipboard_format(vte4::Format::Text);
            }
        }
    }

    fn clipboard_paste(&self) {
        if let Some(uuid) = self.focused_terminal_uuid() {
            if let Some(term) = self.imp().terminals.borrow().get(&uuid) {
                term.vte().paste_clipboard();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;
    use std::time::{Duration, Instant};

    static GTK_INIT: Once = Once::new();

    fn ensure_gtk_init() -> bool {
        let mut success = false;
        GTK_INIT.call_once(|| {
            std::env::set_var("GTK_A11Y", "none");
            success = gtk4::init().is_ok();
        });
        if !success {
            success = std::panic::catch_unwind(|| {
                let _ = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            })
            .is_ok();
        }
        success
    }

    macro_rules! require_display {
        () => {
            if !ensure_gtk_init() {
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

    #[test]
    fn split_rebuild_starts_new_panes_evenly() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", tmp.path());
        std::env::set_var("RTTX_DISABLE_SHELL_SPAWN", "1");

        let app = adw::Application::builder()
            .application_id("com.illya.rttx.window-tests")
            .build();
        app.register(gtk4::gio::Cancellable::NONE).unwrap();

        let window = Window::new(&app);
        window.set_default_size(1200, 800);
        window.present();
        pump_events(100);

        let (session_uuid, t1_uuid) = {
            let state = window.imp().state.borrow();
            let session = &state.sessions[0];
            (
                session.uuid.clone(),
                session.layout.terminal_uuids().into_iter().next().unwrap(),
            )
        };

        window.split_terminal(&t1_uuid, SplitOrientation::Horizontal);
        pump_events(100);

        let t2_uuid = {
            let state = window.imp().state.borrow();
            state.sessions[0]
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
        let outer = root
            .downcast::<gtk4::Paned>()
            .expect("root after split must be a Paned");
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
    fn nested_split_preserves_root_and_unaffected_terminals() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", tmp.path());
        std::env::set_var("RTTX_DISABLE_SHELL_SPAWN", "1");

        let app = adw::Application::builder()
            .application_id("com.illya.rttx.window-identity-tests")
            .build();
        app.register(gtk4::gio::Cancellable::NONE).unwrap();

        let window = Window::new(&app);
        window.set_default_size(1200, 800);
        window.present();
        pump_events(100);

        let (session_uuid, t1_uuid) = {
            let state = window.imp().state.borrow();
            let session = &state.sessions[0];
            (
                session.uuid.clone(),
                session.layout.terminal_uuids().into_iter().next().unwrap(),
            )
        };

        window.split_terminal(&t1_uuid, SplitOrientation::Horizontal);
        pump_events(100);

        let t2_uuid = {
            let state = window.imp().state.borrow();
            state.sessions[0]
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
            t1_before_ptr, t1_after.as_ptr(),
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
}
