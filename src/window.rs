use gtk4::glib;
use gtk4::glib::subclass::prelude::*;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;
use vte4::prelude::*;

use rttx::session::{self, LayoutNode, SessionState, SplitOrientation, WindowState};
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
        let obj: Self = glib::Object::builder()
            .property("application", app)
            .build();
        obj.setup_actions(app);
        obj.setup_signals();
        obj.restore_state();
        obj
    }

    fn setup_actions(&self, app: &adw::Application) {
        let actions: &[(&str, &[&str], fn(&Window))] = &[
            ("new-session",    &["<Ctrl><Shift>t"], |w| w.add_session()),
            ("close-terminal", &["<Ctrl><Shift>w"], |w| w.close_focused_terminal()),
            ("split-h",        &["<Ctrl><Shift>e"], |w| w.split_focused(SplitOrientation::Horizontal)),
            ("split-v",        &["<Ctrl><Shift>o"], |w| w.split_focused(SplitOrientation::Vertical)),
            ("toggle-search",  &["<Ctrl><Shift>f"], |w| w.toggle_focused_search()),
            ("toggle-sidebar", &["<Ctrl><Shift>n"], |w| {
                let sv = &w.imp().split_view;
                sv.set_show_sidebar(!sv.shows_sidebar());
            }),
            ("fullscreen",     &["F11"],            |w| {
                if w.is_fullscreen() { w.unfullscreen() } else { w.fullscreen() }
            }),
            ("next-session",   &["<Ctrl>Tab"],           |w| w.cycle_session(1)),
            ("prev-session",   &["<Ctrl><Shift>Tab"],    |w| w.cycle_session(-1)),
            ("zoom-in",        &["<Ctrl>plus", "<Ctrl>equal"], |w| w.zoom_focused(1)),
            ("zoom-out",       &["<Ctrl>minus"],         |w| w.zoom_focused(-1)),
            ("zoom-reset",     &["<Ctrl>0"],             |w| w.zoom_focused(0)),
            ("copy",           &["<Ctrl><Shift>c"],      |w| w.clipboard_copy()),
            ("paste",          &["<Ctrl><Shift>v"],      |w| w.clipboard_paste()),
        ];

        for &(name, accels, callback) in actions {
            let action = gtk4::gio::SimpleAction::new(name, None);
            let win = self.clone();
            action.connect_activate(move |_, _| callback(&win));
            self.add_action(&action);
            app.set_accels_for_action(&format!("win.{name}"), accels);
        }

        // Input sync toggle (stateful action)
        let sync_action = gtk4::gio::SimpleAction::new_stateful(
            "toggle-input-sync",
            None,
            &false.to_variant(),
        );
        let win = self.clone();
        sync_action.connect_activate(move |action, _| {
            let current = action.state().and_then(|v| v.get::<bool>()).unwrap_or(false);
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
        self.imp()
            .sidebar_list
            .connect_row_selected(move |_, row| {
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
            LayoutNode::Terminal { uuid, cwd, custom_title, .. } => {
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
        let state = imp.state.borrow();
        if let Some(session) = state.sessions.get(index) {
            imp.session_stack.set_visible_child_name(&session.uuid);
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
                            if let Some(sr) = r.child().and_then(|c| c.downcast::<SessionRow>().ok()) {
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
            if let Some(new_layout) =
                state.sessions[idx]
                    .layout
                    .split_terminal(terminal_uuid, orientation)
            {
                state.sessions[idx].layout = new_layout;
                let session_uuid = state.sessions[idx].uuid.clone();
                let session_state = state.sessions[idx].clone();
                drop(state);
                self.rebuild_session_content(&session_uuid, &session_state);
            }
        }
    }

    fn close_terminal(&self, terminal_uuid: &str) {
        let imp = self.imp();

        // Step 1: Update state, extract what we need, release borrow.
        enum Action {
            CloseSession(String),
            Rebuild { session_uuid: String, session_state: SessionState },
        }

        let action = {
            let mut state = imp.state.borrow_mut();
            let session_idx = state.sessions.iter().position(|s| {
                s.layout.terminal_uuids().contains(&terminal_uuid.to_string())
            });
            let Some(idx) = session_idx else { return };

            if state.sessions[idx].layout.terminal_count() <= 1 {
                Action::CloseSession(state.sessions[idx].uuid.clone())
            } else if let Some(new_layout) = state.sessions[idx].layout.remove_terminal(terminal_uuid) {
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
            Action::Rebuild { session_uuid, session_state } => {
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
    fn rebuild_session_content(
        &self,
        session_uuid: &str,
        session_state: &SessionState,
    ) {
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
        {
            let terminals = imp.terminals.borrow();
            for uuid in session_state.layout.terminal_uuids() {
                if let Some(term) = terminals.get(&uuid) {
                    if term.parent().is_some() {
                        term.unparent();
                    }
                }
            }
        }
        // Drop the old content reference so the detached Paned tree can be freed.
        drop(old_content);

        // Step 3: Build new widget tree, reusing existing terminals and only
        // creating + connecting signals for genuinely new ones.
        let win = self.clone();
        let content =
            session::build_layout_widget(&session_state.layout, &|uuid, cwd, _, custom_title| {
                // Reuse existing terminal (already has signal handlers)
                let terminals = win.imp().terminals.borrow();
                if let Some(existing) = terminals.get(uuid) {
                    return existing.clone().upcast();
                }
                drop(terminals);

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

        imp.session_stack
            .add_named(&content, Some(session_uuid));
        imp.session_stack.set_visible_child_name(session_uuid);

        // Ensure the new layout is processed and drawn
        content.queue_allocate();
        content.queue_draw();

        self.update_sidebar_count(session_uuid, session_state.layout.terminal_count());
    }

    fn update_sidebar_count(&self, session_uuid: &str, count: usize) {
        let imp = self.imp();
        let mut idx = 0;
        while let Some(row) = imp.sidebar_list.row_at_index(idx) {
            if let Some(session_row) =
                row.child().and_then(|c| c.downcast::<SessionRow>().ok())
            {
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
        let active_idx = self.imp().sidebar_list.selected_row()
            .map(|r| r.index() as usize)
            .unwrap_or(0);
        if let Some(session) = state.sessions.get_mut(active_idx) {
            session.input_sync = enabled;
        }
    }

    /// Forward input from one terminal to all siblings in the same session
    /// when input sync is enabled.
    fn apply_preferences_to_terminal(&self, term: &TerminalWidget) {
        let prefs = rttx::preferences::load();
        let vte = term.vte();
        let font_desc = gtk4::pango::FontDescription::from_string(&prefs.font);
        vte.set_font(Some(&font_desc));
        vte.set_scrollback_lines(prefs.scrollback_lines);
        vte.set_scroll_on_keystroke(prefs.scroll_on_keystroke);
        vte.set_scroll_on_output(prefs.scroll_on_output);

        // Header visibility
        if !prefs.show_headerbar {
            term.imp().header.set_visible(false);
        }

        // Load and apply color scheme
        if prefs.color_scheme != "default" {
            let mut scheme_path = glib::user_config_dir();
            scheme_path.push(rttx::config::CONFIG_DIR);
            scheme_path.push(rttx::config::SCHEMES_DIR);
            scheme_path.push(format!("{}.json", prefs.color_scheme));
            if let Ok(scheme) = rttx::color_scheme::load_scheme_file(&scheme_path) {
                term.apply_color_scheme(&scheme);
            }
        }
    }

    /// Forward input from one terminal to all siblings in the same session
    /// when input sync is enabled.
    fn forward_input(&self, source_uuid: &str, text: &str) {
        let state = self.imp().state.borrow();
        let session = state.sessions.iter().find(|s| {
            s.input_sync && s.layout.terminal_uuids().contains(&source_uuid.to_string())
        });
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
        if len == 0 { return; }
        let current = imp.sidebar_list.selected_row()
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
            let session = state.sessions.iter_mut().find(|s| {
                s.layout.contains_terminal(uuid_a) && s.layout.contains_terminal(uuid_b)
            });
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
                    1 => { let s = vte.font_scale(); vte.set_font_scale(s * 1.1); }
                    -1 => { let s = vte.font_scale(); vte.set_font_scale(s / 1.1); }
                    _ => vte.set_font_scale(1.0),
                }
            }
        }
    }

    fn notify_process_completed(&self, terminal_uuid: &str, status: i32) {
        let title = self.imp().terminals.borrow()
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
