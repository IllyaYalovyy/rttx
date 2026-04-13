use gtk4::glib;
use gtk4::glib::subclass::prelude::*;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;
use vte4::prelude::*;

use crate::color_scheme;
use crate::commands::{self, CommandRunMode, SavedCommand};
use crate::config;
use crate::host;
use crate::places;
use crate::preferences::{self, Preferences};
use crate::runtime::{
    ConnectionPresentation, ConnectionStatus, RuntimeEndpoint, WorkspaceActionPresentation,
    WorkspacePolicy, connection_icon, pane_description, present_connection_status,
    present_workspace_actions, workspace_connection_summary,
};
use crate::session::{
    self, Direction, LayoutNode, MAX_SPLIT_DEPTH, PaneRecovery, PaneSource, SessionColor,
    SessionState, SplitOrientation, StartupStep, WindowState,
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
        pub sidebar_search_entry: gtk4::SearchEntry,
        pub host_selector: gtk4::DropDown,
        pub host_add_button: gtk4::Button,
        pub host_delete_button: gtk4::Button,
        pub utility_stack: gtk4::Stack,
        pub place_list: gtk4::ListBox,
        pub place_scroll: gtk4::ScrolledWindow,
        pub place_empty: adw::StatusPage,
        pub command_search_entry: gtk4::SearchEntry,
        pub command_list: gtk4::ListBox,
        pub command_scroll: gtk4::ScrolledWindow,
        pub command_empty: adw::StatusPage,
        pub toast_overlay: adw::ToastOverlay,
        pub new_button: gtk4::MenuButton,
        pub connect_button: gtk4::MenuButton,
        pub new_direct_button: gtk4::Button,
        pub state: RefCell<WindowState>,
        pub terminals: RefCell<HashMap<String, TerminalWidget>>,
        pub persistent_terminals: RefCell<HashMap<String, PersistentPaneView>>,
        pub connection_manager: RefCell<Option<crate::daemon_bridge::EndpointConnectionManager>>,
        pub workspace_connection_status: RefCell<HashMap<String, ConnectionStatus>>,
        pub workspace_reconnect_sources: RefCell<HashMap<String, glib::SourceId>>,
        pub focused_terminal_uuid: RefCell<Option<String>>,
        pub workspace_popover: RefCell<Option<gtk4::PopoverMenu>>,
        pub pending_connect_existing: RefCell<Option<crate::host::Host>>,
        pub host_selector_keys: RefCell<Vec<String>>,
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

            self.new_button.set_label("New");
            self.new_button.set_tooltip_text(Some("New workspace"));
            self.new_button.set_icon_name("list-add-symbolic");
            self.new_button.add_css_class("flat");
            self.new_button.update_property(&[gtk4::accessible::Property::Label("New workspace")]);
            header.pack_start(&self.new_button);

            self.connect_button.set_label("Connect");
            self.connect_button.set_tooltip_text(Some("Connect to existing workspace"));
            self.connect_button.set_icon_name("network-server-symbolic");
            self.connect_button.add_css_class("flat");
            self.connect_button
                .update_property(&[gtk4::accessible::Property::Label("Connect to existing")]);
            header.pack_start(&self.connect_button);

            self.new_direct_button.set_label("Direct");
            self.new_direct_button.set_tooltip_text(Some("New direct workspace"));
            self.new_direct_button.add_css_class("flat");
            self.new_direct_button
                .update_property(&[gtk4::accessible::Property::Label("New direct workspace")]);
            header.pack_start(&self.new_direct_button);

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
            menu.append(Some("About rttx"), Some("win.about"));
            menu.append(Some("Preferences"), Some("win.preferences"));
            menu.append(Some("Sync Input"), Some("win.toggle-input-sync"));
            menu.append(Some("Keyboard Shortcuts"), Some("win.show-help-overlay"));
            menu.append(Some("Fullscreen"), Some("win.fullscreen"));
            menu_button.set_menu_model(Some(&menu));

            header.pack_end(&menu_button);

            self.sidebar_list.set_selection_mode(gtk4::SelectionMode::Single);
            self.sidebar_list.add_css_class("navigation-sidebar");
            self.sidebar_list.update_property(&[gtk4::accessible::Property::Label("Workspaces")]);
            self.command_list.set_selection_mode(gtk4::SelectionMode::None);
            self.command_list.add_css_class("boxed-list");
            self.command_list.update_property(&[gtk4::accessible::Property::Label("Commands")]);

            let sidebar_scroll = gtk4::ScrolledWindow::builder()
                .hscrollbar_policy(gtk4::PolicyType::Never)
                .vexpand(true)
                .width_request(200)
                .child(&self.sidebar_list)
                .build();

            // ── Unified search ────────────────────────────────────
            self.sidebar_search_entry.set_placeholder_text(Some("Search…"));
            self.sidebar_search_entry.set_margin_start(12);
            self.sidebar_search_entry.set_margin_end(12);
            self.sidebar_search_entry.set_margin_top(12);

            // ── Host selector ────────────────────────────────────
            let host_model = gtk4::StringList::new(&["Local"]);
            self.host_selector.set_model(Some(&host_model));
            self.host_selector.set_selected(0);
            self.host_selector.set_hexpand(true);
            self.host_selector.update_property(&[gtk4::accessible::Property::Label("Host")]);

            self.host_delete_button.set_icon_name("user-trash-symbolic");
            self.host_delete_button.set_tooltip_text(Some("Delete selected host"));
            self.host_delete_button.add_css_class("flat");
            self.host_delete_button.set_visible(false);

            self.host_add_button.set_icon_name("list-add-symbolic");
            self.host_add_button.set_tooltip_text(Some("Add host"));
            self.host_add_button.add_css_class("flat");

            let host_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
            host_row.set_margin_start(12);
            host_row.set_margin_end(12);
            host_row.set_margin_top(8);
            host_row.append(&self.host_selector);
            host_row.append(&self.host_add_button);
            host_row.append(&self.host_delete_button);

            // ── Places tab ───────────────────────────────────────
            self.place_list.set_selection_mode(gtk4::SelectionMode::None);
            self.place_list.add_css_class("boxed-list");
            self.place_list.update_property(&[gtk4::accessible::Property::Label("Places")]);

            self.place_scroll.set_hscrollbar_policy(gtk4::PolicyType::Never);
            self.place_scroll.set_vexpand(true);
            self.place_scroll.set_margin_start(12);
            self.place_scroll.set_margin_end(12);
            self.place_scroll.set_margin_bottom(12);
            self.place_scroll.set_child(Some(&self.place_list));
            self.place_scroll.set_visible(false);

            self.place_empty.set_icon_name(Some("folder-symbolic"));
            self.place_empty.set_title("No Places");
            self.place_empty.set_description(Some("Save folder paths for quick navigation"));
            self.place_empty.set_vexpand(true);

            let places_page = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            places_page.append(&self.place_scroll);
            places_page.append(&self.place_empty);

            // ── Commands tab ─────────────────────────────────────
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

            let commands_page = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            commands_page.append(&commands_header);
            commands_page.append(&self.command_search_entry);
            commands_page.append(&self.command_scroll);
            commands_page.append(&self.command_empty);

            // ── Tab stack ────────────────────────────────────────
            self.utility_stack.add_titled(&places_page, Some("places"), "Places");
            self.utility_stack.add_titled(&commands_page, Some("commands"), "Commands");

            let utility_switcher =
                gtk4::StackSwitcher::builder().stack(&self.utility_stack).build();
            utility_switcher.set_margin_start(12);
            utility_switcher.set_margin_end(12);
            utility_switcher.set_margin_top(8);

            self.utility_sidebar_box.set_orientation(gtk4::Orientation::Vertical);
            self.utility_sidebar_box.append(&self.sidebar_search_entry);
            self.utility_sidebar_box.append(&host_row);
            self.utility_sidebar_box.append(&utility_switcher);
            self.utility_sidebar_box.append(&self.utility_stack);
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
        self.imp().new_direct_button.connect_clicked(move |_| {
            win.add_direct_session();
        });

        self.setup_host_menu_buttons();

        let win = self.clone();
        self.imp().sidebar_list.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                let index = row.index() as usize;
                win.switch_to_session(index);
            }
        });

        let win = self.clone();
        self.imp().sidebar_search_entry.connect_changed(move |_| {
            win.refresh_place_sidebar();
            win.refresh_command_sidebar();
        });

        let win = self.clone();
        self.imp().command_search_entry.connect_changed(move |_| {
            win.refresh_command_sidebar();
        });

        let win = self.clone();
        self.imp().host_selector.connect_selected_notify(move |_| {
            win.refresh_place_sidebar();
            win.refresh_command_sidebar();
            win.update_host_delete_button_visibility();
        });

        let win = self.clone();
        self.imp().host_delete_button.connect_clicked(move |_| {
            if let Some(key) = win.selected_host_key()
                && key != host::LOCAL_KEY
            {
                win.confirm_delete_host(key);
            }
        });

        let win = self.clone();
        self.imp().host_add_button.connect_clicked(move |_| {
            win.show_add_host_dialog();
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

        self.refresh_place_sidebar();
        self.refresh_command_sidebar();
    }

    fn setup_host_menu_buttons(&self) {
        let new_action =
            gtk4::gio::SimpleAction::new("new-for-host", Some(glib::VariantTy::STRING));
        let win = self.clone();
        new_action.connect_activate(move |_, param| {
            let key: String = param.and_then(glib::Variant::get).unwrap_or_default();
            win.new_workspace_for_host(&key);
        });
        self.add_action(&new_action);

        let connect_action =
            gtk4::gio::SimpleAction::new("connect-for-host", Some(glib::VariantTy::STRING));
        let win = self.clone();
        connect_action.connect_activate(move |_, param| {
            let key: String = param.and_then(glib::Variant::get).unwrap_or_default();
            win.connect_for_host(&key);
        });
        self.add_action(&connect_action);

        let add_host_action = gtk4::gio::SimpleAction::new("add-host", None);
        let win = self.clone();
        add_host_action.connect_activate(move |_, _| {
            win.show_add_host_dialog();
        });
        self.add_action(&add_host_action);

        self.refresh_host_menus();
    }

    pub(super) fn refresh_host_menus(&self) {
        let saved = host::load();

        let mut keys: Vec<String> = vec![host::LOCAL_KEY.into()];
        for h in &saved {
            if !keys.contains(&h.key) {
                keys.push(h.key.clone());
            }
        }
        // Include hosts from active sessions (matching sidebar behavior)
        let state = self.imp().state.borrow();
        for s in &state.sessions {
            let k = s.runtime.endpoint.host_key();
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        drop(state);

        let mut hosts: Vec<host::Host> = keys.iter().map(|k| host::resolve(k, &saved)).collect();
        // Sort remotes alphabetically, keeping Local first
        hosts[1..].sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        let new_menu = gtk4::gio::Menu::new();
        let connect_menu = gtk4::gio::Menu::new();

        for host in &hosts {
            let new_item = gtk4::gio::MenuItem::new(Some(&host.name), None);
            new_item.set_action_and_target_value(
                Some("win.new-for-host"),
                Some(&host.key.to_variant()),
            );
            new_menu.append_item(&new_item);

            let connect_item = gtk4::gio::MenuItem::new(Some(&host.name), None);
            connect_item.set_action_and_target_value(
                Some("win.connect-for-host"),
                Some(&host.key.to_variant()),
            );
            connect_menu.append_item(&connect_item);
        }

        new_menu.append(Some("Add Host\u{2026}"), Some("win.add-host"));
        connect_menu.append(Some("Add Host\u{2026}"), Some("win.add-host"));

        self.imp().new_button.set_menu_model(Some(&new_menu));
        self.imp().connect_button.set_menu_model(Some(&connect_menu));
    }

    fn new_workspace_for_host(&self, host_key: &str) {
        let host = host::resolve(host_key, &host::load());
        crate::new_workspace_dialog::show(self, &host);
    }

    fn connect_for_host(&self, host_key: &str) {
        let host = host::resolve(host_key, &host::load());
        self.request_connect_existing(&host);
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

            let win = self.clone();
            let session_uuid = session_state.uuid.clone();
            let row_for_menu = row.clone();
            row.close_button().connect_clicked(move |_| {
                win.show_workspace_popover_menu(&row_for_menu, &session_uuid);
            });
        } else {
            row.close_button().set_tooltip_text(Some("Close workspace"));

            let win = self.clone();
            let session_uuid = session_state.uuid.clone();
            row.close_button().connect_clicked(move |_| {
                win.confirm_close_session(&session_uuid);
            });
        }

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

    /// Host key for the currently visible workspace session.
    pub(crate) fn visible_session_host_key(&self) -> String {
        let state = self.imp().state.borrow();
        self.imp()
            .session_stack
            .visible_child_name()
            .and_then(|name| state.sessions.iter().find(|s| s.uuid == name.as_str()))
            .map_or_else(|| host::LOCAL_KEY.into(), |s| s.runtime.endpoint.host_key())
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
        self.sync_host_selector_to_workspace(&uuid);
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

    pub(super) fn detach_session(&self, session_uuid: &str) {
        let imp = self.imp();

        let (terminal_uuids, new_index, detach_info) = {
            let mut state = imp.state.borrow_mut();
            if state.sessions.len() <= 1 {
                return;
            }
            let Some(pos) = state.sessions.iter().position(|s| s.uuid == session_uuid) else {
                return;
            };
            let session = &state.sessions[pos];
            let info = session
                .runtime
                .runtime_id
                .as_ref()
                .map(|runtime_id| (session.runtime.endpoint.clone(), runtime_id.clone()));
            let uuids = session.layout.terminal_uuids();
            let session = state.sessions.remove(pos);
            drop(session);
            let new_index = pos.min(state.sessions.len() - 1);
            state.active_session_index = new_index;
            (uuids, new_index, info)
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

        if let Some((endpoint, runtime_id)) = detach_info
            && let Some(manager) = imp.connection_manager.borrow().as_ref()
        {
            manager.detach_runtime(session_uuid, &endpoint, &runtime_id);
            manager.forget_workspace(&endpoint, session_uuid);
        }
        imp.workspace_connection_status.borrow_mut().remove(session_uuid);
        self.clear_workspace_reconnect_countdown(session_uuid);

        if let Some(child) = imp.session_stack.child_by_name(session_uuid) {
            imp.session_stack.remove(&child);
        }
        self.remove_sidebar_row(session_uuid);

        if let Some(row) = imp.sidebar_list.row_at_index(new_index as i32) {
            imp.sidebar_list.select_row(Some(&row));
        }
        self.renumber_session_rows();
    }

    pub(super) fn close_session(&self, session_uuid: &str) {
        let imp = self.imp();

        let (terminal_uuids, new_index, managed_runtime) = {
            let mut state = imp.state.borrow_mut();
            if state.sessions.len() <= 1 {
                let session = state.sessions.iter().find(|s| s.uuid == session_uuid);
                let managed_runtime = session.and_then(|s| {
                    s.uses_managed_runtime()
                        .then(|| (s.runtime.endpoint.clone(), s.runtime.runtime_id.clone()))
                });
                drop(state);
                if let Some((endpoint, runtime_id)) = managed_runtime
                    && let Some(manager) = imp.connection_manager.borrow().as_ref()
                {
                    if let Some(ref runtime_id) = runtime_id {
                        manager.terminate_runtime(session_uuid, &endpoint, runtime_id);
                    }
                    manager.forget_workspace(&endpoint, session_uuid);
                }
                self.close();
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
        self.remove_sidebar_row(session_uuid);

        if let Some(row) = imp.sidebar_list.row_at_index(new_index as i32) {
            imp.sidebar_list.select_row(Some(&row));
        }
        self.renumber_session_rows();
    }

    fn remove_sidebar_row(&self, session_uuid: &str) {
        let imp = self.imp();
        // Clear the stored popover — its parent (the ListBoxRow) is about
        // to be destroyed, so a later unparent() would hit freed memory.
        if let Some(old) = imp.workspace_popover.borrow_mut().take()
            && old.parent().is_some()
        {
            old.unparent();
        }
        let mut idx = 0;
        while let Some(r) = imp.sidebar_list.row_at_index(idx) {
            if let Some(sr) = r.child().and_then(|c| c.downcast::<SessionRow>().ok())
                && sr.uuid() == session_uuid
            {
                imp.sidebar_list.remove(&r);
                return;
            }
            idx += 1;
        }
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
