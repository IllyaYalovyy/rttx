use gtk4::glib;
use gtk4::glib::subclass::prelude::*;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;
use vte4::prelude::*;

use crate::color_scheme;
use crate::preferences;
use crate::session::{self, LayoutNode, SessionState, SplitOrientation, WindowState};
use crate::sidebar::SessionRow;
use crate::terminal::widget::TerminalWidget;
use std::collections::HashMap;

mod imp {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default, Debug)]
    pub struct Window {
        pub split_view: adw::OverlaySplitView,
        pub sidebar_list: gtk4::ListBox,
        pub session_stack: gtk4::Stack,
        pub add_session_button: gtk4::Button,
        pub state: RefCell<WindowState>,
        pub terminals: RefCell<HashMap<String, TerminalWidget>>,
        pub focused_terminal_uuid: RefCell<Option<String>>,
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

            let header = adw::HeaderBar::new();
            let toggle_sidebar = gtk4::ToggleButton::new();
            toggle_sidebar.set_icon_name("sidebar-show-symbolic");
            toggle_sidebar.set_tooltip_text(Some("Toggle sidebar"));
            toggle_sidebar.set_active(true);
            header.pack_start(&toggle_sidebar);

            self.add_session_button.set_tooltip_text(Some("New session"));
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

            self.sidebar_list.set_selection_mode(gtk4::SelectionMode::Single);
            self.sidebar_list.add_css_class("navigation-sidebar");
            self.sidebar_list.update_property(&[gtk4::accessible::Property::Label("Sessions")]);

            let sidebar_scroll = gtk4::ScrolledWindow::builder()
                .hscrollbar_policy(gtk4::PolicyType::Never)
                .vexpand(true)
                .width_request(200)
                .child(&self.sidebar_list)
                .build();

            self.session_stack.set_hexpand(true);
            self.session_stack.set_vexpand(true);

            self.split_view.set_sidebar(Some(&sidebar_scroll));
            self.split_view.set_content(Some(&self.session_stack));
            self.split_view.set_show_sidebar(true);
            self.split_view.set_collapsed(false);
            self.split_view.set_min_sidebar_width(180.0);
            self.split_view.set_max_sidebar_width(300.0);

            self.split_view
                .bind_property("show-sidebar", &toggle_sidebar, "active")
                .bidirectional()
                .sync_create()
                .build();

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
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget, gtk4::Native, gtk4::Root, gtk4::ShortcutManager,
                    gtk4::gio::ActionGroup, gtk4::gio::ActionMap;
}

impl Window {
    #[must_use]
    pub fn new(app: &adw::Application) -> Self {
        let obj: Self = glib::Object::builder().property("application", app).build();
        obj.setup_actions(app);
        obj.setup_signals();
        obj.load_state();
        obj
    }

    fn load_state(&self) {
        let state = session::load_window_state();
        let active_index = state.active_session_index;
        let is_maximized = state.is_maximized;
        let width = state.width;
        let height = state.height;

        self.imp().state.replace(state.clone());

        for session in &state.sessions {
            self.build_session(session);
        }

        if is_maximized {
            self.maximize();
        } else {
            self.set_default_size(width, height);
        }

        if let Some(row) = self.imp().sidebar_list.row_at_index(active_index as i32) {
            self.imp().sidebar_list.select_row(Some(&row));
        }
    }

    pub fn save_state(&self) {
        let imp = self.imp();
        let mut state = imp.state.borrow().clone();

        let (width, height) = self.default_size();
        state.width = width;
        state.height = height;
        state.is_maximized = self.is_maximized();

        let active_index = imp.sidebar_list.selected_row().map_or(0, |r| r.index() as usize);
        state.active_session_index = active_index;

        for session in &mut state.sessions {
            if let Some(content) = imp.session_stack.child_by_name(&session.uuid) {
                session::capture_paned_ratios(&mut session.layout, &content);
            }
        }

        {
            let terminals = imp.terminals.borrow();
            for session in &mut state.sessions {
                for node_uuid in session.layout.terminal_uuids() {
                    if let Some(term) = terminals.get(&node_uuid) {
                        if let LayoutNode::Terminal { ref mut cwd, .. } = session.layout {
                            *cwd = term.current_directory();
                        }
                    }
                }
            }
        }

        if let Err(e) = session::save_window_state(&state) {
            log::error!("Failed to save window state: {e}");
        }
    }

    fn setup_actions(&self, app: &adw::Application) {
        type ActionCallback = fn(&Window);
        let actions: &[(&str, &[&str], ActionCallback)] = &[
            ("close-terminal", &["<Ctrl><Shift>W"], Self::close_focused_terminal),
            ("split-horizontal", &["<Ctrl><Shift>E"], |w| {
                w.split_focused(SplitOrientation::Horizontal);
            }),
            ("split-vertical", &["<Ctrl><Shift>O"], |w| {
                w.split_focused(SplitOrientation::Vertical);
            }),
            ("search", &["<Ctrl><Shift>F"], Self::toggle_focused_search),
            ("copy", &["<Ctrl><Shift>C"], Self::clipboard_copy),
            ("paste", &["<Ctrl><Shift>V"], Self::clipboard_paste),
            ("prev-session", &["<Ctrl><Shift>Tab"], |w| w.cycle_session(-1)),
            ("next-session", &["<Ctrl>Tab"], |w| w.cycle_session(1)),
            ("toggle-sidebar", &["<Ctrl><Shift>N"], |w| {
                w.imp().split_view.set_show_sidebar(!w.imp().split_view.shows_sidebar());
            }),
            ("fullscreen", &["F11"], |w| {
                if w.is_fullscreen() {
                    w.unfullscreen();
                } else {
                    w.fullscreen();
                }
            }),
            ("zoom-in", &["<Ctrl>plus", "<Ctrl>equal"], |w| w.zoom_focused(1)),
            ("zoom-out", &["<Ctrl>minus"], |w| w.zoom_focused(-1)),
            ("zoom-reset", &["<Ctrl>0"], |w| w.zoom_focused(0)),
            ("new-session", &["<Ctrl><Shift>T"], Self::add_session),
        ];

        for (name, accels, callback) in actions {
            let action = gtk4::gio::SimpleAction::new(name, None);
            let win = self.clone();
            let cb = *callback;
            action.connect_activate(move |_, _| {
                cb(&win);
            });
            self.add_action(&action);
            app.set_accels_for_action(&format!("win.{name}"), accels);
        }

        let sync_action =
            gtk4::gio::SimpleAction::new_stateful("toggle-input-sync", None, &false.to_variant());
        let win = self.clone();
        sync_action.connect_activate(move |action, _| {
            let state = action.state().unwrap();
            let new_val = !state.get::<bool>().unwrap();
            action.set_state(&new_val.to_variant());
            win.set_input_sync(new_val);
        });
        self.add_action(&sync_action);
        app.set_accels_for_action("win.toggle-input-sync", &["<Ctrl><Shift>i"]);

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

        let win = self.clone();
        adw::StyleManager::default().connect_dark_notify(move |_| {
            win.reapply_terminal_preferences();
        });
    }

    fn build_session(&self, session_state: &SessionState) {
        let imp = self.imp();
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

        let win = self.clone();
        let content = session::build_layout_widget(
            &session_state.layout,
            &move |uuid, cwd, _, custom_title| {
                let existing = {
                    let terminals = win.imp().terminals.borrow();
                    terminals.get(uuid).cloned()
                };
                if let Some(existing) = existing {
                    if existing.parent().is_some() {
                        existing.unparent();
                    }
                    return existing.upcast();
                }

                let term = TerminalWidget::new(uuid, cwd);
                if let Some(title) = custom_title {
                    term.set_title(title);
                    term.imp().custom_title.replace(Some(title.to_string()));
                }
                win.connect_terminal_signals(&term);
                win.imp().terminals.borrow_mut().insert(uuid.to_string(), term.clone());
                term.upcast()
            },
        );

        imp.session_stack.add_named(&content, Some(&session_state.uuid));
        Self::schedule_apply_paned_ratios(&content, &session_state.layout);
        self.update_sidebar_count(&session_state.uuid, session_state.layout.terminal_count());
    }

    fn connect_terminal_signals(&self, term: &TerminalWidget) {
        self.apply_preferences_to_terminal(term);

        let win = self.clone();
        let uuid = term.uuid();
        let focus_controller = gtk4::EventControllerFocus::new();
        focus_controller.connect_enter(move |_| {
            win.imp().focused_terminal_uuid.replace(Some(uuid.clone()));
        });
        term.vte().add_controller(focus_controller);

        let win = self.clone();
        let uuid = term.uuid();
        term.vte().connect_commit(move |_, text, _| {
            win.forward_input(&uuid, text);
        });

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
        let session_state = SessionState::new(format!("Session {count}"));
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
        if let Some(action) = self.lookup_action("toggle-input-sync") {
            if let Ok(action) = action.downcast::<gtk4::gio::SimpleAction>() {
                action.set_state(&input_sync.to_variant());
            }
        }
    }

    fn close_session(&self, session_uuid: &str) {
        let imp = self.imp();

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

        {
            let terminals = imp.terminals.borrow();
            for uuid in &terminal_uuids {
                if let Some(term) = terminals.get(uuid) {
                    term.disconnect_child_exited();
                }
            }
        }

        {
            let mut terminals = imp.terminals.borrow_mut();
            for uuid in &terminal_uuids {
                terminals.remove(uuid);
            }
        }

        if let Some(child) = imp.session_stack.child_by_name(session_uuid) {
            imp.session_stack.remove(&child);
        }
        if let Some(row) = imp.sidebar_list.row_at_index({
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
        }) {
            imp.sidebar_list.remove(&row);
        }

        if let Some(row) = imp.sidebar_list.row_at_index(new_index as i32) {
            imp.sidebar_list.select_row(Some(&row));
        }
    }

    fn split_terminal(&self, terminal_uuid: &str, orientation: SplitOrientation) {
        let imp = self.imp();
        let mut state = imp.state.borrow_mut();

        let session_idx = state
            .sessions
            .iter()
            .position(|s| s.layout.terminal_uuids().contains(&terminal_uuid.to_string()));

        if let Some(idx) = session_idx {
            if let Some((new_layout, new_terminal_uuid)) =
                state.sessions[idx].layout.split_terminal_with_new_uuid(terminal_uuid, orientation)
            {
                state.sessions[idx].layout = new_layout;
                let session_uuid = state.sessions[idx].uuid.clone();
                let session_state = state.sessions[idx].clone();
                drop(state);
                if self.split_terminal_in_place(
                    &session_uuid,
                    terminal_uuid,
                    &new_terminal_uuid,
                    orientation,
                ) {
                    self.update_sidebar_count(&session_uuid, session_state.layout.terminal_count());
                } else {
                    self.rebuild_session_content(&session_uuid, &session_state);
                }
            }
        }
    }

    fn close_terminal(&self, terminal_uuid: &str) {
        #[derive(Debug)]
        enum Action {
            CloseSession(String),
            Rebuild { session_uuid: String, session_state: SessionState },
        }
        let imp = self.imp();

        let action = {
            let mut state = imp.state.borrow_mut();
            let session_idx = state
                .sessions
                .iter()
                .position(|s| s.layout.terminal_uuids().contains(&terminal_uuid.to_string()));
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

        match action {
            Action::CloseSession(uuid) => self.close_session(&uuid),
            Action::Rebuild { session_uuid, session_state } => {
                if let Some(term) = imp.terminals.borrow().get(terminal_uuid) {
                    term.disconnect_child_exited();
                }
                imp.terminals.borrow_mut().remove(terminal_uuid);
                self.rebuild_session_content(&session_uuid, &session_state);
            }
        }
    }

    fn rebuild_session_content(&self, session_uuid: &str, session_state: &SessionState) {
        let imp = self.imp();

        let old_content = imp.session_stack.child_by_name(session_uuid);
        if let Some(ref old) = old_content {
            imp.session_stack.remove(old);
        }

        if let Some(ref old) = old_content {
            Self::detach_terminals_from_detached_tree(old);
        }
        drop(old_content);

        let win = self.clone();
        let content = session::build_layout_widget(
            &session_state.layout,
            &move |uuid, cwd, _, custom_title| {
                let existing = {
                    let terminals = win.imp().terminals.borrow();
                    terminals.get(uuid).cloned()
                };
                if let Some(existing) = existing {
                    if existing.parent().is_some() {
                        existing.unparent();
                    }
                    return existing.upcast();
                }

                let term = TerminalWidget::new(uuid, cwd);
                if let Some(title) = custom_title {
                    term.set_title(title);
                    term.imp().custom_title.replace(Some(title.to_string()));
                }
                win.connect_terminal_signals(&term);
                win.imp().terminals.borrow_mut().insert(uuid.to_string(), term.clone());
                term.upcast()
            },
        );

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
        if let LayoutNode::Split { orientation, .. } = layout {
            let idle_layout = layout.clone();
            glib::idle_add_local_once(glib::clone!(
                #[weak]
                content,
                move || {
                    session::apply_paned_ratios(&idle_layout, &content);
                }
            ));

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
        imp.terminals.borrow_mut().insert(new_terminal_uuid.to_string(), new_term.clone());

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

        let win_weak = self.downgrade();
        let target_uuid_str = target_uuid.to_string();
        let new_terminal_uuid_str = new_terminal_uuid.to_string();
        let target_clone = target.clone();
        let new_term_clone = new_term;
        let branch_layout_clone = branch_layout.clone();

        let build_branch = move || {
            if win_weak.upgrade().is_some() {
                session::build_layout_widget(&branch_layout_clone, &|uuid, _, _, _| {
                    if uuid == target_uuid_str {
                        target_clone.clone().upcast()
                    } else if uuid == new_terminal_uuid_str {
                        new_term_clone.clone().upcast()
                    } else {
                        unreachable!("split branch builder requested unexpected uuid {uuid}");
                    }
                })
            } else {
                gtk4::Box::new(gtk4::Orientation::Vertical, 0).upcast()
            }
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

        let target_widget = target.upcast::<gtk4::Widget>();
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

    fn set_input_sync(&self, enabled: bool) {
        let mut state = self.imp().state.borrow_mut();
        let active_idx = self.imp().sidebar_list.selected_row().map_or(0, |r| r.index() as usize);
        if let Some(session) = state.sessions.get_mut(active_idx) {
            session.input_sync = enabled;
        }
    }

    fn apply_preferences_to_terminal(&self, term: &TerminalWidget) {
        let prefs = preferences::load();
        let vte = term.vte();
        let font_desc = gtk4::pango::FontDescription::from_string(&prefs.font);
        vte.set_font(Some(&font_desc));
        vte.set_scrollback_lines(prefs.scrollback_lines);
        vte.set_scroll_on_keystroke(prefs.scroll_on_keystroke);
        vte.set_scroll_on_output(prefs.scroll_on_output);
        vte.set_audible_bell(prefs.audible_bell);

        term.imp().header.set_visible(prefs.show_headerbar);

        let is_dark = adw::StyleManager::default().is_dark();
        let effective_name = prefs.effective_color_scheme_name(is_dark);
        if let Some(scheme) =
            color_scheme::load_color_scheme_by_name(effective_name).or_else(|| {
                let fallback = if is_dark {
                    color_scheme::BUILTIN_DARK_SCHEME_NAME
                } else {
                    color_scheme::BUILTIN_LIGHT_SCHEME_NAME
                };
                color_scheme::load_color_scheme_by_name(fallback)
            })
        {
            term.apply_color_scheme(&scheme);
        }
    }

    pub(crate) fn reapply_terminal_preferences(&self) {
        let terminals: Vec<TerminalWidget> =
            self.imp().terminals.borrow().values().cloned().collect();
        for term in terminals {
            self.apply_preferences_to_terminal(&term);
        }
    }

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
        let current = imp.sidebar_list.selected_row().map_or(0, |r| r.index());
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
            .map_or_else(|| "Terminal".into(), |t| t.title_label().label().to_string());

        let body = if status == 0 {
            format!("\"{title}\" completed successfully")
        } else {
            format!("\"{title}\" exited with status {status}")
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
    fn save_and_restart_restores_user_resized_pane_ratios() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", tmp.path());
        std::env::set_var("RTTX_DISABLE_SHELL_SPAWN", "1");

        let app = adw::Application::builder()
            .application_id("com.illya.rttx.restore-ratios-tests")
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
        let restored_ratio =
            restored_paned.position() as f64 / restored_paned.width().max(1) as f64;
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
    fn save_and_restart_restores_nested_user_resized_pane_ratios() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", tmp.path());
        std::env::set_var("RTTX_DISABLE_SHELL_SPAWN", "1");

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
            state.sessions[0]
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
            (session.uuid.clone(), session.layout.terminal_uuids().into_iter().next().unwrap())
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
}
