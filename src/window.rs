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
        obj.setup_signals();
        obj.restore_state();
        obj
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
            LayoutNode::Terminal { uuid, cwd, .. } => {
                if let Some(term) = self.imp().terminals.borrow().get(uuid.as_str()) {
                    *cwd = term.current_directory();
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
            session::build_layout_widget(&session_state.layout, &|uuid, cwd, _| {
                let term = TerminalWidget::new(uuid, cwd);
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
        let handler_id = term.vte().connect_child_exited(move |_, _| {
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

        // Step 1: Unparent all existing terminals that belong to this session
        // so they can be reparented into the new Paned tree.
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

        // Step 2: Remove old container from the stack (now safe — terminals
        // have been detached).
        if let Some(old) = imp.session_stack.child_by_name(session_uuid) {
            imp.session_stack.remove(&old);
        }

        // Step 3: Build new widget tree, reusing existing terminals and only
        // creating + connecting signals for genuinely new ones.
        let win = self.clone();
        let content =
            session::build_layout_widget(&session_state.layout, &|uuid, cwd, _| {
                // Reuse existing terminal (already has signal handlers)
                let terminals = win.imp().terminals.borrow();
                if let Some(existing) = terminals.get(uuid) {
                    return existing.clone().upcast();
                }
                drop(terminals);

                // New terminal — create and connect signals
                let term = TerminalWidget::new(uuid, cwd);
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
}
