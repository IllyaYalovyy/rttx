use gtk4::glib;
use gtk4::glib::subclass::prelude::*;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;
use vte4::prelude::*;

use crate::bookmarks::Bookmark;
use crate::color_scheme;
use crate::commands::{self, CommandRunMode, SavedCommand};
use crate::config;
use crate::preferences::{self, Preferences};
use crate::runtime::{
    ConnectionPresentation, ConnectionStatus, RuntimeEndpoint, WorkspaceActionPresentation,
    WorkspacePolicy, connection_icon, pane_description, present_connection_status,
    present_workspace_actions, workspace_connection_summary,
};
use crate::session::{
    self, Direction, LayoutNode, MAX_SPLIT_DEPTH, PaneRecovery, PaneSource, PaneTarget,
    SessionColor, SessionState, SplitOrientation, StartupStep, WindowState,
};
use crate::sidebar::SessionRow;
use crate::terminal::handle::TerminalHandle;
use crate::terminal::persistent_widget::PersistentPaneView;
use crate::terminal::widget::TerminalWidget;
use crate::workspace_state::{EndpointEventTransition, WorkspacePaneRestore};
use std::collections::HashMap;

mod actions;
mod dialogs;
mod input;
mod runtime;
mod sidebar;
mod terminal;

mod imp {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default, Debug)]
    pub struct Window {
        pub left_paned: gtk4::Paned,
        pub right_paned: gtk4::Paned,
        pub devel_badge: gtk4::Label,
        pub sidebar_list: gtk4::ListBox,
        pub session_stack: gtk4::Stack,
        pub utility_sidebar_box: gtk4::Box,
        pub bookmark_search_entry: gtk4::SearchEntry,
        pub bookmark_list: gtk4::ListBox,
        pub bookmark_scroll: gtk4::ScrolledWindow,
        pub bookmark_empty: adw::StatusPage,
        pub command_search_entry: gtk4::SearchEntry,
        pub command_list: gtk4::ListBox,
        pub command_scroll: gtk4::ScrolledWindow,
        pub command_empty: adw::StatusPage,
        pub toast_overlay: adw::ToastOverlay,
        pub add_session_button: gtk4::Button,
        pub state: RefCell<WindowState>,
        pub terminals: RefCell<HashMap<String, TerminalWidget>>,
        pub persistent_terminals: RefCell<HashMap<String, PersistentPaneView>>,
        pub connection_manager: RefCell<Option<crate::daemon_bridge::EndpointConnectionManager>>,
        pub workspace_connection_status: RefCell<HashMap<String, ConnectionStatus>>,
        pub workspace_reconnect_sources: RefCell<HashMap<String, glib::SourceId>>,
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
            obj.set_title(Some(config::display_name()));
            obj.set_icon_name(Some(config::icon_name()));

            let header = adw::HeaderBar::new();
            let toggle_sidebar = gtk4::ToggleButton::new();
            toggle_sidebar.set_icon_name("sidebar-show-symbolic");
            toggle_sidebar.set_tooltip_text(Some("Toggle sidebar"));
            toggle_sidebar.set_active(true);
            header.pack_start(&toggle_sidebar);

            self.add_session_button.set_icon_name("list-add-symbolic");
            self.add_session_button.set_tooltip_text(Some("New persistent workspace"));
            header.pack_start(&self.add_session_button);

            if let Some(label) = config::badge_label() {
                self.devel_badge.set_label(label);
                self.devel_badge.add_css_class("caption");
                self.devel_badge.add_css_class("accent");
                self.devel_badge.add_css_class("pill");
                self.devel_badge
                    .set_tooltip_text(Some("Development mode uses a separate app profile"));
                header.pack_start(&self.devel_badge);
            }

            let toggle_utility_sidebar = gtk4::ToggleButton::with_label("Tools");
            toggle_utility_sidebar.set_tooltip_text(Some("Toggle tools sidebar"));
            toggle_utility_sidebar.set_active(true);
            toggle_utility_sidebar.add_css_class("flat");
            header.pack_end(&toggle_utility_sidebar);

            let menu_button = gtk4::MenuButton::new();
            menu_button.set_icon_name("open-menu-symbolic");

            let menu = gtk4::gio::Menu::new();
            menu.append(Some("New Persistent Workspace"), Some("win.new-session"));
            menu.append(Some("New Ephemeral Workspace"), Some("win.new-ephemeral-workspace"));
            menu.append(Some("New Remote Workspace"), Some("win.new-remote-workspace"));
            menu.append(Some("Attach to Remote Runtime"), Some("win.browse-remote-runtimes"));
            menu.append(Some("About rttx"), Some("win.about"));
            menu.append(Some("Bookmark This Workspace"), Some("win.bookmark-session"));
            menu.append(Some("Preferences"), Some("win.preferences"));
            menu.append(Some("Sync Input"), Some("win.toggle-input-sync"));
            menu.append(Some("Keyboard Shortcuts"), Some("win.show-help-overlay"));
            menu.append(Some("Fullscreen"), Some("win.fullscreen"));
            menu_button.set_menu_model(Some(&menu));

            header.pack_end(&menu_button);

            self.sidebar_list.set_selection_mode(gtk4::SelectionMode::Single);
            self.sidebar_list.add_css_class("navigation-sidebar");
            self.sidebar_list.update_property(&[gtk4::accessible::Property::Label("Workspaces")]);
            self.bookmark_list.set_selection_mode(gtk4::SelectionMode::None);
            self.bookmark_list.add_css_class("boxed-list");
            self.bookmark_list.update_property(&[gtk4::accessible::Property::Label("Bookmarks")]);
            self.command_list.set_selection_mode(gtk4::SelectionMode::None);
            self.command_list.add_css_class("boxed-list");
            self.command_list.update_property(&[gtk4::accessible::Property::Label("Commands")]);

            let sidebar_scroll = gtk4::ScrolledWindow::builder()
                .hscrollbar_policy(gtk4::PolicyType::Never)
                .vexpand(true)
                .width_request(200)
                .child(&self.sidebar_list)
                .build();

            let add_bookmark_button = gtk4::Button::builder()
                .icon_name("list-add-symbolic")
                .tooltip_text("New bookmark")
                .action_name("win.add-bookmark")
                .build();
            let utility_header = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
            utility_header.set_margin_start(12);
            utility_header.set_margin_end(12);
            utility_header.set_margin_top(12);
            let utility_title = gtk4::Label::new(Some("Bookmarks"));
            utility_title.set_xalign(0.0);
            utility_title.set_hexpand(true);
            utility_title.add_css_class("title-4");
            utility_header.append(&utility_title);
            utility_header.append(&add_bookmark_button);

            self.bookmark_search_entry.set_placeholder_text(Some("Search bookmarks"));
            self.bookmark_search_entry.set_margin_start(12);
            self.bookmark_search_entry.set_margin_end(12);
            self.bookmark_search_entry.set_margin_top(12);
            self.bookmark_search_entry.set_margin_bottom(12);

            self.bookmark_scroll.set_hscrollbar_policy(gtk4::PolicyType::Never);
            self.bookmark_scroll.set_vexpand(true);
            self.bookmark_scroll.set_margin_start(12);
            self.bookmark_scroll.set_margin_end(12);
            self.bookmark_scroll.set_margin_bottom(12);
            self.bookmark_scroll.set_child(Some(&self.bookmark_list));
            self.bookmark_scroll.set_visible(false);

            self.bookmark_empty.set_icon_name(Some("bookmarks-symbolic"));
            self.bookmark_empty.set_title("No Bookmarks");
            self.bookmark_empty.set_description(Some(
                "Add a bookmark to quickly open folders, connect to SSH hosts, or attach to tmux sessions",
            ));
            self.bookmark_empty.set_vexpand(true);

            let add_command_button = gtk4::Button::builder()
                .icon_name("list-add-symbolic")
                .tooltip_text("New command")
                .action_name("win.add-command")
                .build();
            let commands_header = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
            commands_header.set_margin_start(12);
            commands_header.set_margin_end(12);
            commands_header.set_margin_top(12);
            let commands_title = gtk4::Label::new(Some("Commands"));
            commands_title.set_xalign(0.0);
            commands_title.set_hexpand(true);
            commands_title.add_css_class("title-4");
            commands_header.append(&commands_title);
            commands_header.append(&add_command_button);

            self.command_search_entry.set_placeholder_text(Some("Search commands"));
            self.command_search_entry.set_margin_start(12);
            self.command_search_entry.set_margin_end(12);
            self.command_search_entry.set_margin_top(12);
            self.command_search_entry.set_margin_bottom(12);

            self.command_scroll.set_hscrollbar_policy(gtk4::PolicyType::Never);
            self.command_scroll.set_vexpand(true);
            self.command_scroll.set_margin_start(12);
            self.command_scroll.set_margin_end(12);
            self.command_scroll.set_margin_bottom(12);
            self.command_scroll.set_child(Some(&self.command_list));
            self.command_scroll.set_visible(false);

            self.command_empty.set_icon_name(Some("system-run-symbolic"));
            self.command_empty.set_title("No Commands");
            self.command_empty.set_description(Some(
                "Save frequently used commands to run or insert from the sidebar",
            ));
            self.command_empty.set_vexpand(true);

            let templates_placeholder =
                gtk4::Label::new(Some("Workspace templates will live here."));
            templates_placeholder.set_wrap(true);
            templates_placeholder.set_margin_start(18);
            templates_placeholder.set_margin_end(18);
            templates_placeholder.set_margin_top(18);
            templates_placeholder.set_margin_bottom(18);

            let bookmarks_page = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            bookmarks_page.append(&utility_header);
            bookmarks_page.append(&self.bookmark_search_entry);
            bookmarks_page.append(&self.bookmark_scroll);
            bookmarks_page.append(&self.bookmark_empty);

            let commands_page = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            commands_page.append(&commands_header);
            commands_page.append(&self.command_search_entry);
            commands_page.append(&self.command_scroll);
            commands_page.append(&self.command_empty);

            let utility_stack = gtk4::Stack::new();
            utility_stack.add_titled(&bookmarks_page, Some("bookmarks"), "Bookmarks");
            utility_stack.add_titled(&commands_page, Some("commands"), "Commands");
            utility_stack.add_titled(&templates_placeholder, Some("templates"), "Templates");

            let utility_switcher = gtk4::StackSwitcher::builder().stack(&utility_stack).build();
            utility_switcher.set_margin_start(12);
            utility_switcher.set_margin_end(12);
            utility_switcher.set_margin_top(12);

            self.utility_sidebar_box.set_orientation(gtk4::Orientation::Vertical);
            self.utility_sidebar_box.append(&utility_switcher);
            self.utility_sidebar_box.append(&utility_stack);
            self.utility_sidebar_box.set_width_request(320);

            self.session_stack.set_hexpand(true);
            self.session_stack.set_vexpand(true);

            self.right_paned.set_orientation(gtk4::Orientation::Horizontal);
            self.right_paned.set_start_child(Some(&self.session_stack));
            self.right_paned.set_end_child(Some(&self.utility_sidebar_box));
            self.right_paned.set_resize_start_child(true);
            self.right_paned.set_resize_end_child(false);
            self.right_paned.set_shrink_start_child(false);
            self.right_paned.set_shrink_end_child(false);

            self.toast_overlay.set_child(Some(&self.right_paned));

            self.left_paned.set_orientation(gtk4::Orientation::Horizontal);
            self.left_paned.set_start_child(Some(&sidebar_scroll));
            self.left_paned.set_end_child(Some(&self.toast_overlay));
            self.left_paned.set_resize_start_child(false);
            self.left_paned.set_resize_end_child(true);
            self.left_paned.set_shrink_start_child(false);
            self.left_paned.set_shrink_end_child(false);
            self.left_paned.set_position(220);

            let utility_panel = self.utility_sidebar_box.clone();
            toggle_sidebar.connect_toggled(move |btn| {
                sidebar_scroll.set_visible(btn.is_active());
            });
            toggle_utility_sidebar.connect_toggled(move |btn| {
                utility_panel.set_visible(btn.is_active());
            });

            let main_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            main_box.append(&header);
            main_box.append(&self.left_paned);

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
        let active_index = state.active_session_index.min(state.sessions.len().saturating_sub(1));
        let is_maximized = state.is_maximized;
        let width = state.width;
        let height = state.height;
        let left_sidebar_width = state.left_sidebar_width;
        let right_sidebar_width = state.right_sidebar_width;

        self.imp().state.replace(state.clone());

        for session in &state.sessions {
            self.build_session(session, true);
        }

        if is_maximized {
            self.maximize();
        } else {
            self.set_default_size(width, height);
        }

        self.imp().left_paned.set_position(left_sidebar_width);

        let right_paned = self.imp().right_paned.clone();
        right_paned.connect_realize(move |paned| {
            let total = paned.width();
            if total > 0 {
                paned.set_position((total - right_sidebar_width).max(0));
            }
        });

        if let Some(row) = self.imp().sidebar_list.row_at_index(active_index as i32) {
            self.imp().sidebar_list.select_row(Some(&row));
        }

        if state.needs_inventory_bootstrap(&RuntimeEndpoint::Local)
            && crate::daemon::default_socket_path().exists()
        {
            let win = self.clone();
            glib::idle_add_local_once(move || {
                if win.ensure_connection_manager()
                    && let Some(manager) = win.imp().connection_manager.borrow().as_ref()
                {
                    manager.refresh_inventory(&RuntimeEndpoint::Local);
                }
            });
        }
    }

    pub fn save_state(&self) {
        let imp = self.imp();
        let mut state = imp.state.borrow().clone();

        let (width, height) = self.default_size();
        state.width = width;
        state.height = height;
        state.is_maximized = self.is_maximized();
        state.left_sidebar_width = imp.left_paned.position();
        let right_total = imp.right_paned.width();
        let right_pos = imp.right_paned.position();
        if right_total > 0 && imp.utility_sidebar_box.is_visible() {
            state.right_sidebar_width = (right_total - right_pos).max(0);
        }

        let active_index = imp.sidebar_list.selected_row().map_or(0, |r| r.index() as usize);
        state.active_session_index = active_index;
        if let Some(focused_terminal_uuid) = self.focused_terminal_uuid()
            && let Some(session) = state
                .sessions
                .iter_mut()
                .find(|session| session.layout.contains_terminal(&focused_terminal_uuid))
        {
            session.active_terminal_uuid = Some(focused_terminal_uuid);
        }

        for session in &mut state.sessions {
            if !session.is_zoomed()
                && let Some(content) = imp.session_stack.child_by_name(&session.uuid)
            {
                session::capture_paned_ratios(&mut session.layout, &content);
            }
            session.prune_recovery();
            session.normalize_active_terminal();
            session.sync_legacy_mode_from_runtime();
            session.zoomed_terminal_uuid = None;
        }

        {
            let terminals = imp.terminals.borrow();
            for session in &mut state.sessions {
                for node_uuid in session.layout.terminal_uuids() {
                    if let Some(term) = terminals.get(&node_uuid) {
                        session.layout.set_terminal_cwd(&node_uuid, term.current_directory());
                        session.layout.set_terminal_custom_title(&node_uuid, term.custom_title());
                    }
                }
            }
        }

        {
            let panes = imp.persistent_terminals.borrow();
            for session in &mut state.sessions {
                for node_uuid in session.layout.terminal_uuids() {
                    if let Some(pane) = panes.get(&node_uuid) {
                        if let Some(cwd) = pane.current_directory() {
                            session.layout.set_terminal_cwd(&node_uuid, Some(cwd));
                        }
                        session.layout.set_terminal_custom_title(&node_uuid, pane.custom_title());
                    }
                }
            }
        }

        if let Err(e) = session::save_window_state(&state) {
            log::error!("Failed to save window state: {e}");
        }
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
        self.imp().bookmark_search_entry.connect_changed(move |_| {
            win.refresh_bookmark_sidebar();
        });

        let win = self.clone();
        self.imp().command_search_entry.connect_changed(move |_| {
            win.refresh_command_sidebar();
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

        self.refresh_bookmark_sidebar();
        self.refresh_command_sidebar();
    }

    fn append_session_row(&self, session_state: &SessionState) {
        let imp = self.imp();
        let row = SessionRow::new(&session_state.uuid, &session_state.name);

        let initial_status = if session_state.uses_managed_runtime() {
            ConnectionStatus::Connecting
        } else {
            ConnectionStatus::Connected
        };
        let icon = connection_icon(
            &session_state.runtime.endpoint,
            &initial_status,
            session_state.uses_managed_runtime(),
        );
        row.set_connection_icon(&icon);

        if session_state.uses_managed_runtime() {
            row.set_managed_actions_style();
            row.close_button().set_tooltip_text(Some("Workspace actions"));
        } else {
            row.close_button().set_tooltip_text(Some("Close workspace"));
        }

        let win = self.clone();
        let session_uuid = session_state.uuid.clone();
        row.close_button().connect_clicked(move |_| {
            win.confirm_close_session(&session_uuid);
        });

        let win = self.clone();
        let session_uuid = session_state.uuid.clone();
        let row_for_rename = row.clone();
        let rename_gesture = gtk4::GestureClick::new();
        rename_gesture.set_button(1);
        rename_gesture.connect_released(move |gesture, n_press, _, _| {
            if n_press == 2 {
                win.show_rename_session_popover(&row_for_rename, &session_uuid);
                gesture.set_state(gtk4::EventSequenceState::Claimed);
            }
        });
        row.add_controller(rename_gesture);

        let drag_source = gtk4::DragSource::new();
        drag_source.set_actions(gtk4::gdk::DragAction::MOVE);
        let drag_uuid = session_state.uuid.clone();
        drag_source.connect_prepare(move |_, _, _| {
            Some(gtk4::gdk::ContentProvider::for_value(&format!("session:{drag_uuid}").to_value()))
        });
        row.add_controller(drag_source);

        let drop_target = gtk4::DropTarget::new(glib::Type::STRING, gtk4::gdk::DragAction::MOVE);
        let win = self.clone();
        let target_uuid = session_state.uuid.clone();
        drop_target.connect_drop(move |_, value, _, _| {
            if let Ok(payload) = value.get::<String>()
                && let Some(source_uuid) = payload.strip_prefix("session:")
                && source_uuid != target_uuid
            {
                win.reorder_session(source_uuid, &target_uuid);
                return true;
            }
            false
        });
        row.add_controller(drop_target);

        let win = self.clone();
        let session_uuid_for_ctx = session_state.uuid.clone();
        let ctx_gesture = gtk4::GestureClick::new();
        ctx_gesture.set_button(3);
        ctx_gesture.connect_released(move |gesture, _, _, _| {
            gesture.set_state(gtk4::EventSequenceState::Claimed);
            win.show_sidebar_context_menu(&session_uuid_for_ctx);
        });
        row.add_controller(ctx_gesture);

        let list_row = gtk4::ListBoxRow::new();
        list_row.set_child(Some(&row));
        imp.sidebar_list.append(&list_row);
    }

    fn sync_sidebar_to_visible_session(&self) {
        let imp = self.imp();
        let Some(visible_uuid) = imp.session_stack.visible_child_name() else {
            return;
        };
        let already_synced = imp
            .sidebar_list
            .selected_row()
            .and_then(|r| r.child())
            .and_then(|c| c.downcast::<SessionRow>().ok())
            .is_some_and(|sr| sr.uuid() == visible_uuid.as_str());
        if already_synced {
            return;
        }
        let state = imp.state.borrow();
        if let Some(idx) = state.sessions.iter().position(|s| s.uuid == visible_uuid.as_str()) {
            drop(state);
            if let Some(row) = imp.sidebar_list.row_at_index(idx as i32) {
                imp.sidebar_list.select_row(Some(&row));
            }
        }
    }

    fn renumber_session_rows(&self) {
        let imp = self.imp();
        let mut idx = 0;
        while let Some(row) = imp.sidebar_list.row_at_index(idx) {
            if let Some(session_row) = row.child().and_then(|c| c.downcast::<SessionRow>().ok()) {
                session_row.set_position(idx as usize);
            }
            idx += 1;
        }
    }

    fn build_session(&self, session_state: &SessionState, auto_connect_managed: bool) {
        let imp = self.imp();
        self.append_session_row(session_state);

        let content = self.build_session_content(session_state);

        imp.session_stack.add_named(&content, Some(&session_state.uuid));
        session::schedule_initial_paned_ratios(&content, &session_state.layout);
        self.refresh_sidebar_subtitle(&session_state.uuid);
        self.renumber_session_rows();

        if auto_connect_managed && session_state.uses_managed_runtime() {
            self.connect_managed_workspace(session_state);
        }
    }

    fn build_session_content(&self, session_state: &SessionState) -> gtk4::Widget {
        if let Some(ref zoomed_uuid) = session_state.zoomed_terminal_uuid {
            let zoomed_layout = LayoutNode::Terminal {
                uuid: zoomed_uuid.clone(),
                profile: None,
                cwd: session_state.layout.terminal_cwd(zoomed_uuid),
                custom_title: session_state.layout.terminal_custom_title(zoomed_uuid),
            };
            let win = self.clone();
            return session::build_layout_widget(&zoomed_layout, &move |spec| {
                win.materialize_terminal(session_state, spec.uuid, spec.cwd, spec.custom_title)
            });
        }
        let win = self.clone();
        session::build_layout_widget(&session_state.layout, &move |spec| {
            win.materialize_terminal(session_state, spec.uuid, spec.cwd, spec.custom_title)
        })
    }

    fn resolve_default_session_folder(&self) -> Option<String> {
        let prefs = preferences::load();
        match prefs.default_session_folder {
            preferences::DefaultSessionFolder::Home => None,
            preferences::DefaultSessionFolder::CurrentSession => {
                let terminal_uuid = {
                    let state = self.imp().state.borrow();
                    let active = state.active_session_index;
                    state.sessions.get(active).and_then(|session| {
                        session
                            .active_terminal_uuid
                            .clone()
                            .or_else(|| session.layout.terminal_uuids().into_iter().next())
                    })
                };
                terminal_uuid.and_then(|uuid| {
                    self.terminal_handle(&uuid).and_then(|terminal| terminal.current_directory())
                })
            }
            preferences::DefaultSessionFolder::Custom(ref path) => {
                if path.is_empty() {
                    None
                } else {
                    Some(path.clone())
                }
            }
        }
    }

    fn next_session_color(&self) -> SessionColor {
        let count = self.imp().state.borrow().sessions.len();
        SessionColor::ALL[count % SessionColor::ALL.len()]
    }

    pub(crate) fn new_session_from_bookmark(&self, bookmark: &Bookmark) {
        let imp = self.imp();
        let initial_cwd = bookmark
            .pane_target()
            .as_ref()
            .and_then(PaneTarget::initial_cwd)
            .map(str::to_string)
            .or_else(|| bookmark.session_initial_cwd().map(str::to_string));

        let mut session_state = if let Some(host) = bookmark.remote_host() {
            SessionState::new_managed_remote(
                bookmark.name.clone(),
                host,
                WorkspacePolicy::Persistent,
                initial_cwd,
            )
        } else {
            SessionState::new_with_initial_cwd(bookmark.name.clone(), initial_cwd)
        };
        session_state.color = self.next_session_color();
        let session_uuid = session_state.uuid.clone();
        let terminal_uuid = session_state.layout.terminal_uuids().into_iter().next().unwrap();
        imp.state.borrow_mut().sessions.push(session_state.clone());
        self.build_session(&session_state, true);

        let index = imp.state.borrow().sessions.len() as i32 - 1;
        if let Some(row) = imp.sidebar_list.row_at_index(index) {
            imp.sidebar_list.select_row(Some(&row));
        }

        if session_state.runtime.is_managed() {
            self.set_workspace_connection_status(
                &session_state.uuid,
                &ConnectionStatus::Connecting,
            );
            self.connect_managed_workspace(&session_state);
        } else {
            self.setup_bookmark_terminal(&terminal_uuid, bookmark);
        }
        self.imp().session_stack.set_visible_child_name(&session_uuid);
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
        if let Some(row) = imp.sidebar_list.row_at_index(index as i32)
            && let Some(session_row) = row.child().and_then(|c| c.downcast::<SessionRow>().ok())
        {
            session_row.clear_activity();
        }
        self.focus_session_terminal(&uuid);
        if let Some(action) = self.lookup_action("toggle-input-sync")
            && let Ok(action) = action.downcast::<gtk4::gio::SimpleAction>()
        {
            action.set_state(&input_sync.to_variant());
        }
    }

    fn focus_session_terminal(&self, session_uuid: &str) {
        let target = {
            let state = self.imp().state.borrow();
            let Some(session) = state.sessions.iter().find(|session| session.uuid == session_uuid)
            else {
                return;
            };

            let preferred_uuid = self
                .focused_terminal_uuid()
                .filter(|uuid| {
                    session
                        .active_terminal_uuid
                        .as_deref()
                        .is_none_or(|active_uuid| active_uuid == uuid)
                })
                .or_else(|| session.active_terminal_uuid.clone())
                .filter(|uuid| session.layout.contains_terminal(uuid))
                .or_else(|| session.layout.terminal_uuids().into_iter().next());

            let Some(preferred_uuid) = preferred_uuid else {
                return;
            };
            drop(state);
            self.terminal_handle(&preferred_uuid).map(|terminal| (preferred_uuid, terminal))
        };

        let Some((target_uuid, terminal)) = target else {
            return;
        };
        let win = self.clone();
        glib::idle_add_local_once(move || {
            if terminal.grab_focus() {
                win.set_focused_terminal(Some(&target_uuid));
            }
        });
    }

    fn reorder_session(&self, source_uuid: &str, target_uuid: &str) {
        let imp = self.imp();
        let visible_uuid = imp.session_stack.visible_child_name().map(|n| n.to_string());

        {
            let mut state = imp.state.borrow_mut();
            let Some(src) = state.sessions.iter().position(|s| s.uuid == source_uuid) else {
                return;
            };
            let Some(tgt) = state.sessions.iter().position(|s| s.uuid == target_uuid) else {
                return;
            };
            let session = state.sessions.remove(src);
            state.sessions.insert(tgt, session);
        }

        // Rebuild sidebar rows to reflect new order.
        while let Some(row) = imp.sidebar_list.row_at_index(0) {
            imp.sidebar_list.remove(&row);
        }
        let sessions: Vec<_> = {
            let state = imp.state.borrow();
            state.sessions.clone()
        };
        for session_state in &sessions {
            self.append_session_row(session_state);
        }

        // Re-apply connection status subtitles lost during row rebuild.
        let statuses = imp.workspace_connection_status.borrow().clone();
        for (workspace_id, status) in &statuses {
            self.refresh_workspace_row_status(workspace_id, status);
        }

        // Re-select the previously visible session.
        if let Some(uuid) = &visible_uuid {
            let state = imp.state.borrow();
            if let Some(idx) = state.sessions.iter().position(|s| s.uuid == *uuid) {
                drop(state);
                if let Some(row) = imp.sidebar_list.row_at_index(idx as i32) {
                    imp.sidebar_list.select_row(Some(&row));
                }
            }
        }
        self.renumber_session_rows();
    }

    fn close_session(&self, session_uuid: &str) {
        let imp = self.imp();

        let (terminal_uuids, new_index, managed_runtime) = {
            let mut state = imp.state.borrow_mut();
            if state.sessions.len() <= 1 {
                return;
            }
            let Some(pos) = state.sessions.iter().position(|s| s.uuid == session_uuid) else {
                return;
            };
            let session = state.sessions.remove(pos);
            let uuids = session.layout.terminal_uuids();
            let managed_runtime = if session.uses_managed_runtime() {
                Some((session.runtime.endpoint.clone(), session.runtime.runtime_id))
            } else {
                None
            };
            let new_index = pos.min(state.sessions.len() - 1);
            state.active_session_index = new_index;
            (uuids, new_index, managed_runtime)
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

        {
            let mut panes = imp.persistent_terminals.borrow_mut();
            for uuid in &terminal_uuids {
                panes.remove(uuid);
            }
        }

        if let Some((endpoint, runtime_id)) = managed_runtime
            && let Some(manager) = imp.connection_manager.borrow().as_ref()
        {
            if let Some(ref runtime_id) = runtime_id {
                manager.terminate_runtime(session_uuid, &endpoint, runtime_id);
            }
            manager.forget_workspace(&endpoint, session_uuid);
            imp.workspace_connection_status.borrow_mut().remove(session_uuid);

            // Prevent inventory resurrection of the terminated runtime.
            if let Some(runtime_id) = runtime_id {
                imp.state.borrow_mut().dismissed_runtime_ids.insert(runtime_id);
            }
        }
        self.clear_workspace_reconnect_countdown(session_uuid);

        if let Some(child) = imp.session_stack.child_by_name(session_uuid) {
            imp.session_stack.remove(&child);
        }
        if let Some(row) = imp.sidebar_list.row_at_index({
            let mut idx = 0;
            loop {
                match imp.sidebar_list.row_at_index(idx) {
                    Some(r) => {
                        if let Some(sr) = r.child().and_then(|c| c.downcast::<SessionRow>().ok())
                            && sr.uuid() == session_uuid
                        {
                            break idx;
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
        self.renumber_session_rows();
    }
}

fn terminal_is_in_background_session(
    terminal_uuid: &str,
    visible_session_uuid: Option<&str>,
    state: &WindowState,
) -> bool {
    visible_session_uuid.is_none_or(|visible| {
        !state
            .sessions
            .iter()
            .find(|s| s.uuid == visible)
            .is_some_and(|s| s.layout.contains_terminal(terminal_uuid))
    })
}

/// Notification tier for a process exit event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationTier {
    /// Terminal is in the visible session — no notification needed.
    Suppress,
    /// Window is focused but terminal is in a background session — use toast.
    Toast,
    /// Window is not focused — use desktop notification.
    Desktop,
}

#[must_use]
fn notification_tier(
    terminal_uuid: &str,
    visible_session_uuid: Option<&str>,
    window_active: bool,
    state: &WindowState,
) -> NotificationTier {
    if let Some(visible_uuid) = visible_session_uuid
        && state
            .sessions
            .iter()
            .find(|s| s.uuid == visible_uuid)
            .is_some_and(|s| s.layout.contains_terminal(terminal_uuid))
    {
        return NotificationTier::Suppress;
    }
    if window_active { NotificationTier::Toast } else { NotificationTier::Desktop }
}

fn preferred_command_target_uuid(
    focused_terminal_uuid: Option<&str>,
    visible_session_uuid: Option<&str>,
    state: &WindowState,
) -> Option<String> {
    if let Some(focused_terminal_uuid) = focused_terminal_uuid {
        return Some(focused_terminal_uuid.to_string());
    }

    if let Some(visible_session_uuid) = visible_session_uuid
        && let Some(session) =
            state.sessions.iter().find(|session| session.uuid == visible_session_uuid)
    {
        return session.layout.terminal_uuids().into_iter().next();
    }

    state
        .sessions
        .get(state.active_session_index)
        .and_then(|session| session.layout.terminal_uuids().into_iter().next())
}

#[cfg(test)]
#[allow(
    clippy::default_trait_access,
    clippy::doc_markdown,
    clippy::field_reassign_with_default,
    clippy::match_wildcard_for_single_variants,
    clippy::redundant_clone,
    clippy::redundant_closure_for_method_calls
)]
mod tests;
