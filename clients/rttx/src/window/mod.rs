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

mod runtime;

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
                let panel = w.imp().left_paned.start_child().expect("left sidebar panel");
                panel.set_visible(!panel.is_visible());
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
            ("toggle-pane-zoom", &["<Ctrl><Shift>Z"], Self::toggle_pane_zoom),
            ("new-session", &["<Ctrl><Shift>T"], Self::add_session),
            ("new-ephemeral-workspace", &["<Ctrl><Shift><Alt>T"], Self::add_ephemeral_session),
            ("new-remote-workspace", &[], Self::show_new_remote_workspace_dialog),
            ("browse-remote-runtimes", &[], Self::show_browse_remote_runtimes_dialog),
            ("toggle-utility-sidebar", &["<Ctrl><Shift>B"], |w| {
                let sidebar = &w.imp().utility_sidebar_box;
                sidebar.set_visible(!sidebar.is_visible());
            }),
            ("bookmark-session", &[], Self::do_bookmark_active_session),
            ("add-bookmark", &[], |w| {
                crate::bookmarks_window::show_form(w, None);
            }),
            ("add-command", &[], |w| {
                crate::commands_window::show_form(w, None);
            }),
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

        let edit_bookmark_action =
            gtk4::gio::SimpleAction::new("edit-bookmark", Some(glib::VariantTy::STRING));
        let win = self.clone();
        edit_bookmark_action.connect_activate(move |_, param| {
            let uuid: String = param.and_then(glib::Variant::get).unwrap_or_default();
            let bookmarks = crate::bookmarks::load();
            if let Some(bookmark) = bookmarks.iter().find(|b| b.uuid == uuid) {
                crate::bookmarks_window::show_form(&win, Some(bookmark));
            }
        });
        self.add_action(&edit_bookmark_action);

        let delete_bookmark_action =
            gtk4::gio::SimpleAction::new("delete-bookmark", Some(glib::VariantTy::STRING));
        let win = self.clone();
        delete_bookmark_action.connect_activate(move |_, param| {
            let uuid: String = param.and_then(glib::Variant::get).unwrap_or_default();
            if !uuid.is_empty() {
                win.confirm_delete_bookmark(uuid);
            }
        });
        self.add_action(&delete_bookmark_action);

        let edit_command_action =
            gtk4::gio::SimpleAction::new("edit-command", Some(glib::VariantTy::STRING));
        let win = self.clone();
        edit_command_action.connect_activate(move |_, param| {
            let uuid: String = param.and_then(glib::Variant::get).unwrap_or_default();
            let all_commands = commands::load();
            if let Some(command) = all_commands.iter().find(|c| c.uuid == uuid) {
                crate::commands_window::show_form(&win, Some(command));
            }
        });
        self.add_action(&edit_command_action);

        let delete_command_action =
            gtk4::gio::SimpleAction::new("delete-command", Some(glib::VariantTy::STRING));
        let win = self.clone();
        delete_command_action.connect_activate(move |_, param| {
            let uuid: String = param.and_then(glib::Variant::get).unwrap_or_default();
            if !uuid.is_empty() {
                win.confirm_delete_command(uuid);
            }
        });
        self.add_action(&delete_command_action);

        let about_action = gtk4::gio::SimpleAction::new("about", None);
        let win = self.clone();
        about_action.connect_activate(move |_, _| {
            win.show_about_window();
        });
        self.add_action(&about_action);

        // Pane navigation — shortcuts are configurable via preferences.
        {
            let nav_keys = crate::preferences::load().pane_navigation_keys;
            let (left, right, up, down) = nav_keys.accels();
            let nav_actions: &[(&str, &str, Direction)] = &[
                ("navigate-left", left, Direction::Left),
                ("navigate-right", right, Direction::Right),
                ("navigate-up", up, Direction::Up),
                ("navigate-down", down, Direction::Down),
            ];
            for (name, accel, direction) in nav_actions {
                let action = gtk4::gio::SimpleAction::new(name, None);
                let win = self.clone();
                let dir = *direction;
                action.connect_activate(move |_, _| win.navigate_focused(dir));
                self.add_action(&action);
                app.set_accels_for_action(&format!("win.{name}"), &[accel]);
            }
        }

        for number in 1_u8..=9 {
            let action_name = format!("switch-to-session-{number}");
            let win = self.clone();
            let action = gtk4::gio::SimpleAction::new(&action_name, None);
            action.connect_activate(move |_, _| {
                win.switch_to_session_number(number as usize);
            });
            self.add_action(&action);

            let accel = format!("<Alt>{number}");
            app.set_accels_for_action(&format!("win.{action_name}"), &[accel.as_str()]);
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
        row.close_button().set_tooltip_text(Some(if session_state.uses_managed_runtime() {
            "Workspace actions"
        } else {
            "Close workspace"
        }));
        row.set_color(session_state.color);

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
            return session::build_layout_widget(
                &zoomed_layout,
                &move |uuid, cwd, _, custom_title| {
                    win.materialize_terminal(session_state, uuid, cwd, custom_title)
                },
            );
        }
        let win = self.clone();
        session::build_layout_widget(&session_state.layout, &move |uuid, cwd, _, custom_title| {
            win.materialize_terminal(session_state, uuid, cwd, custom_title)
        })
    }

    fn materialize_terminal(
        &self,
        session_state: &SessionState,
        uuid: &str,
        cwd: Option<&str>,
        custom_title: Option<&str>,
    ) -> gtk4::Widget {
        if session_state.uses_managed_runtime() {
            return self.materialize_persistent_terminal(session_state, uuid, custom_title);
        }

        let existing = {
            let terminals = self.imp().terminals.borrow();
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
            term.set_custom_title(Some(title));
        }
        self.connect_terminal_signals(&term);
        self.imp().terminals.borrow_mut().insert(uuid.to_string(), term.clone());
        self.initialize_terminal_recovery(&term, session_state, uuid);
        term.upcast()
    }

    /// Create a `PersistentPaneView` for a daemon-backed session.
    fn materialize_persistent_terminal(
        &self,
        session_state: &SessionState,
        uuid: &str,
        custom_title: Option<&str>,
    ) -> gtk4::Widget {
        let existing = {
            let panes = self.imp().persistent_terminals.borrow();
            panes.get(uuid).cloned()
        };
        if let Some(existing) = existing {
            if existing.parent().is_some() {
                existing.unparent();
            }
            return existing.upcast();
        }

        let daemon_session_id = session_state.runtime.runtime_id.as_deref().unwrap_or_default();
        let pane_view = PersistentPaneView::new(uuid, daemon_session_id);
        if let Some(title) = custom_title {
            pane_view.set_custom_title(Some(title));
        }
        self.apply_preferences_to_persistent_pane(&pane_view);
        self.connect_managed_pane(session_state, &pane_view);
        self.imp().persistent_terminals.borrow_mut().insert(uuid.to_string(), pane_view.clone());
        pane_view.upcast()
    }

    fn initialize_terminal_recovery(
        &self,
        term: &TerminalWidget,
        session_state: &SessionState,
        terminal_uuid: &str,
    ) {
        let Some(recovery) = session_state.recovery_for(terminal_uuid) else {
            term.ensure_shell_spawned_when_ready();
            return;
        };
        if recovery.target.is_none() && recovery.startup.is_empty() {
            term.ensure_shell_spawned_when_ready();
            return;
        }
        self.attempt_recovery_for_terminal(term, recovery);
    }

    fn show_sidebar_context_menu(&self, session_uuid: &str) {
        let is_remote = {
            let state = self.imp().state.borrow();
            state
                .sessions
                .iter()
                .find(|s| s.uuid == session_uuid)
                .is_some_and(|s| matches!(s.runtime.endpoint, RuntimeEndpoint::Remote { .. }))
        };
        if !is_remote {
            return;
        }

        let menu = gtk4::gio::Menu::new();
        menu.append(Some("Edit Connection…"), Some("win.edit-connection"));
        menu.append(Some("Retry Connection"), Some("win.retry-connection"));

        let popover = gtk4::PopoverMenu::from_model(Some(&menu));
        popover.set_has_arrow(true);

        let row = self
            .sidebar_row_for_uuid(session_uuid)
            .and_then(|r| r.parent())
            .unwrap_or_else(|| self.imp().sidebar_list.clone().upcast());
        popover.set_parent(&row);
        popover.connect_closed(gtk4::prelude::WidgetExt::unparent);

        let win = self.clone();
        let uuid = session_uuid.to_string();
        let edit_action = gtk4::gio::SimpleAction::new("edit-connection", None);
        edit_action.connect_activate(move |_, _| {
            win.show_edit_workspace_connection_dialog(&uuid);
        });

        let win2 = self.clone();
        let uuid2 = session_uuid.to_string();
        let retry_action = gtk4::gio::SimpleAction::new("retry-connection", None);
        retry_action.connect_activate(move |_, _| {
            win2.retry_workspace_connection(&uuid2);
        });

        self.add_action(&edit_action);
        self.add_action(&retry_action);
        popover.popup();
    }

    fn sidebar_row_for_uuid(&self, session_uuid: &str) -> Option<SessionRow> {
        let imp = self.imp();
        let mut idx = 0;
        while let Some(row) = imp.sidebar_list.row_at_index(idx) {
            if let Some(sr) = row.child().and_then(|c| c.downcast::<SessionRow>().ok())
                && sr.uuid() == session_uuid
            {
                return Some(sr);
            }
            idx += 1;
        }
        None
    }

    fn show_rename_session_popover(&self, row: &SessionRow, session_uuid: &str) {
        let current_name = {
            let state = self.imp().state.borrow();
            let Some(session) = state.sessions.iter().find(|session| session.uuid == session_uuid)
            else {
                return;
            };
            session.name.clone()
        };

        let popover = gtk4::Popover::new();
        popover.set_has_arrow(true);
        popover.set_position(gtk4::PositionType::Bottom);
        // Parent the popover on the wrapper ListBoxRow (row's parent), not the
        // SessionRow itself, because SessionRow is a ListBoxRow subclass that
        // isn't directly in the ListBox — attaching to it causes GTK to fail
        // the `box != NULL` assertion when grabbing focus.
        let popover_parent = row.parent().unwrap_or_else(|| row.clone().upcast::<gtk4::Widget>());
        popover.set_parent(&popover_parent);

        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
        content.set_margin_start(12);
        content.set_margin_end(12);
        content.set_margin_top(12);
        content.set_margin_bottom(12);
        let entry = gtk4::Entry::new();
        entry.set_hexpand(true);
        entry.set_text(&current_name);
        content.append(&entry);

        let actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        let cancel_button = gtk4::Button::with_label("Cancel");
        let rename_button = gtk4::Button::with_label("Rename");
        rename_button.add_css_class("suggested-action");
        actions.append(&cancel_button);
        actions.append(&rename_button);
        content.append(&actions);
        popover.set_child(Some(&content));

        let win = self.clone();
        let session_uuid = session_uuid.to_string();
        let popover_for_commit = popover.clone();
        let entry_for_commit = entry.clone();
        let commit = move || {
            let name = entry_for_commit.text().trim().to_string();
            if !name.is_empty() {
                win.rename_session(&session_uuid, &name);
            }
            popover_for_commit.popdown();
        };
        let commit_for_button = commit.clone();
        rename_button.connect_clicked(move |_| commit_for_button());
        entry.connect_activate(move |_| commit());

        let popover_for_cancel = popover.clone();
        cancel_button.connect_clicked(move |_| {
            popover_for_cancel.popdown();
        });

        popover.connect_closed(|popover| {
            popover.unparent();
        });

        popover.popup();
        entry.grab_focus();
    }

    fn rename_session(&self, session_uuid: &str, new_name: &str) {
        {
            let mut state = self.imp().state.borrow_mut();
            let Some(session) =
                state.sessions.iter_mut().find(|session| session.uuid == session_uuid)
            else {
                return;
            };
            session.name = new_name.to_string();
            session.user_renamed = true;
        }

        let mut idx = 0;
        while let Some(row) = self.imp().sidebar_list.row_at_index(idx) {
            if let Some(session_row) =
                row.child().and_then(|child| child.downcast::<SessionRow>().ok())
                && session_row.uuid() == session_uuid
            {
                session_row.set_session_name(new_name);
                return;
            }
            idx += 1;
        }
    }

    fn connect_terminal_signals(&self, term: &TerminalWidget) {
        {
            let prefs = preferences::load();
            let font_desc = gtk4::pango::FontDescription::from_string(&prefs.font);
            let is_dark = adw::StyleManager::default().is_dark();
            let effective_name = prefs.effective_color_scheme_name(is_dark);
            let scheme = color_scheme::load_color_scheme_by_name(effective_name).or_else(|| {
                let fallback = if is_dark {
                    color_scheme::BUILTIN_DARK_SCHEME_NAME
                } else {
                    color_scheme::BUILTIN_LIGHT_SCHEME_NAME
                };
                color_scheme::load_color_scheme_by_name(fallback)
            });
            Self::apply_preferences_to_terminal(term, &prefs, &font_desc, scheme.as_ref());
        }

        let win = self.clone();
        let uuid = term.uuid();
        let focus_controller = gtk4::EventControllerFocus::new();
        focus_controller.connect_enter(move |_| {
            win.set_focused_terminal(Some(&uuid));
            let session_uuid = {
                let mut state = win.imp().state.borrow_mut();
                let session = state
                    .sessions
                    .iter_mut()
                    .find(|session| session.layout.contains_terminal(&uuid));
                if let Some(session) = session {
                    session.active_terminal_uuid = Some(uuid.clone());
                    Some(session.uuid.clone())
                } else {
                    None
                }
            };
            if let Some(session_uuid) = session_uuid {
                win.refresh_sidebar_subtitle(&session_uuid);
            }
        });
        term.vte().add_controller(focus_controller);

        let win = self.clone();
        let uuid = term.uuid();
        term.vte().connect_commit(move |_, text, _| {
            win.forward_input(&uuid, text);
        });

        let bell_term = term.clone();
        term.vte().connect_bell(move |_| {
            bell_term.flash_bell();
        });

        let win = self.clone();
        let uuid = term.uuid();
        term.vte().connect_contents_changed(move |_| {
            win.mark_session_activity(&uuid);
        });

        let win = self.clone();
        let uuid = term.uuid();
        term.vte().connect_window_title_changed(move |_| {
            win.refresh_sidebar_subtitle_if_active(&uuid);
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
            if let Ok(source_uuid) = value.get::<String>()
                && source_uuid != target_uuid
            {
                win.swap_terminals(&source_uuid, &target_uuid);
                return true;
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
        let recoverable_term = term.clone();
        let handler_id = term.vte().connect_child_exited(move |_, status| {
            recoverable_term.reset_terminal_state();
            if win.handle_recoverable_terminal_exit(&recoverable_term, &uuid, status) {
                return;
            }
            let visible_session = win.imp().session_stack.visible_child_name();
            let state = win.imp().state.borrow();
            let in_background =
                terminal_is_in_background_session(&uuid, visible_session.as_deref(), &state);
            drop(state);
            if in_background {
                win.notify_process_completed(&uuid, status);
            }
            win.close_terminal(&uuid);
        });
        term.imp().child_exited_handler.replace(Some(handler_id));

        let win = self.clone();
        let term_for_retry = term.clone();
        term.recovery_retry_button().connect_clicked(move |_| {
            win.retry_terminal_recovery(&term_for_retry);
        });
    }

    fn show_about_window(&self) {
        let about = adw::AboutWindow::new();
        about.set_transient_for(Some(self));
        about.set_application_name(config::display_name());
        about.set_application_icon(config::icon_name());
        about.set_version(&format!("{} ({})", env!("CARGO_PKG_VERSION"), env!("GIT_HASH")));
        about.set_developer_name(config::DEVELOPER_NAME);
        about.set_developers(&[config::DEVELOPER_NAME]);
        about.set_website(config::PROJECT_WEBSITE);
        about.set_issue_url(config::ISSUE_TRACKER);
        about.set_license_type(gtk4::License::Gpl30);
        about.present();
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

    pub(crate) fn execute_bookmark(&self, bookmark: &Bookmark) {
        let Some(terminal_uuid) = self.command_target_terminal_uuid() else {
            return;
        };

        let command = self.resolve_bookmark_command(bookmark);
        let Some(command) = command else {
            return;
        };
        self.set_terminal_recovery(
            &terminal_uuid,
            PaneRecovery {
                source: PaneSource::Bookmark { name: bookmark.name.clone() },
                target: None,
                startup: vec![StartupStep::SendText { text: command.clone(), execute: true }],
            },
        );
        self.send_input_to_terminal(&terminal_uuid, &format!("{command}\n"));
    }

    fn resolve_bookmark_command(&self, bookmark: &Bookmark) -> Option<String> {
        let bookmark_host = bookmark.remote_host();
        if let Some(bh) = bookmark_host {
            let session_host = self.visible_session_remote_host();
            if session_host.as_deref() == Some(bh) {
                return bookmark.remote_command().or_else(|| bookmark.command());
            }
        }
        bookmark.command()
    }

    fn visible_session_remote_host(&self) -> Option<String> {
        let state = self.imp().state.borrow();
        let visible = self.imp().session_stack.visible_child_name()?;
        let session = state.sessions.iter().find(|s| s.uuid == visible.as_str())?;
        match &session.runtime.endpoint {
            RuntimeEndpoint::Remote { host } if session.runtime.is_managed() => Some(host.clone()),
            _ => None,
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

    fn command_target_terminal_uuid(&self) -> Option<String> {
        let visible_session_uuid =
            self.imp().session_stack.visible_child_name().map(|name| name.to_string());
        {
            let state = self.imp().state.borrow();
            preferred_command_target_uuid(
                self.focused_terminal_uuid().as_deref(),
                visible_session_uuid.as_deref(),
                &state,
            )
        }
    }

    pub(crate) fn refresh_bookmark_sidebar(&self) {
        let imp = self.imp();
        while let Some(row) = imp.bookmark_list.row_at_index(0) {
            imp.bookmark_list.remove(&row);
        }

        let query = imp.bookmark_search_entry.text();
        for bookmark in crate::bookmarks::load()
            .into_iter()
            .filter(|bookmark| crate::bookmarks::matches_query(bookmark, query.as_str()))
        {
            let action_row = adw::ActionRow::new();
            action_row.set_title(&bookmark.name);
            action_row.set_subtitle(&bookmark.summary());
            action_row.set_activatable(true);

            let new_session_button = gtk4::Button::builder()
                .icon_name(bookmark.new_workspace_icon())
                .tooltip_text(bookmark.new_workspace_tooltip())
                .valign(gtk4::Align::Center)
                .build();
            new_session_button.add_css_class("flat");

            let uuid = bookmark.uuid.clone();
            let edit_item = gtk4::gio::MenuItem::new(Some("Edit"), None);
            edit_item
                .set_action_and_target_value(Some("win.edit-bookmark"), Some(&uuid.to_variant()));
            let delete_item = gtk4::gio::MenuItem::new(Some("Delete"), None);
            delete_item
                .set_action_and_target_value(Some("win.delete-bookmark"), Some(&uuid.to_variant()));
            let menu = gtk4::gio::Menu::new();
            menu.append_item(&edit_item);
            menu.append_item(&delete_item);
            let more_button = gtk4::MenuButton::builder()
                .icon_name("view-more-symbolic")
                .tooltip_text("More options")
                .valign(gtk4::Align::Center)
                .menu_model(&menu)
                .build();
            more_button.add_css_class("flat");

            action_row.add_suffix(&new_session_button);
            action_row.add_suffix(&more_button);

            let drag_source = gtk4::DragSource::new();
            drag_source.set_actions(gtk4::gdk::DragAction::MOVE);
            let drag_uuid = bookmark.uuid.clone();
            drag_source.connect_prepare(move |_, _, _| {
                Some(gtk4::gdk::ContentProvider::for_value(&drag_uuid.to_value()))
            });
            action_row.add_controller(drag_source);

            let drop_target =
                gtk4::DropTarget::new(glib::Type::STRING, gtk4::gdk::DragAction::MOVE);
            let win = self.clone();
            let target_uuid = bookmark.uuid.clone();
            drop_target.connect_drop(move |_, value, _, _| {
                if let Ok(source_uuid) = value.get::<String>()
                    && source_uuid != target_uuid
                {
                    win.reorder_bookmark(&source_uuid, &target_uuid);
                    return true;
                }
                false
            });
            action_row.add_controller(drop_target);

            imp.bookmark_list.append(&action_row);

            let win = self.clone();
            let bookmark_for_run = bookmark.clone();
            action_row.connect_activated(move |_| {
                win.execute_bookmark(&bookmark_for_run);
            });

            let win = self.clone();
            new_session_button.connect_clicked(move |_| {
                win.new_session_from_bookmark(&bookmark);
            });
        }

        let is_empty = imp.bookmark_list.row_at_index(0).is_none();
        imp.bookmark_scroll.set_visible(!is_empty);
        imp.bookmark_empty.set_visible(is_empty);
    }

    pub(crate) fn refresh_command_sidebar(&self) {
        let imp = self.imp();
        while let Some(row) = imp.command_list.row_at_index(0) {
            imp.command_list.remove(&row);
        }

        let query = imp.command_search_entry.text();
        for command in commands::load()
            .into_iter()
            .filter(|command| commands::matches_query(command, query.as_str()))
        {
            let action_row = adw::ActionRow::new();
            action_row.set_title(&command.title);
            action_row.set_subtitle(&command.preview());
            action_row.set_activatable(true);

            let insert_button = gtk4::Button::builder()
                .icon_name("insert-text-symbolic")
                .tooltip_text("Insert into current pane")
                .valign(gtk4::Align::Center)
                .build();
            insert_button.add_css_class("flat");

            let uuid = command.uuid.clone();
            let edit_item = gtk4::gio::MenuItem::new(Some("Edit"), None);
            edit_item
                .set_action_and_target_value(Some("win.edit-command"), Some(&uuid.to_variant()));
            let delete_item = gtk4::gio::MenuItem::new(Some("Delete"), None);
            delete_item
                .set_action_and_target_value(Some("win.delete-command"), Some(&uuid.to_variant()));
            let menu = gtk4::gio::Menu::new();
            menu.append_item(&edit_item);
            menu.append_item(&delete_item);
            let more_button = gtk4::MenuButton::builder()
                .icon_name("view-more-symbolic")
                .tooltip_text("More options")
                .valign(gtk4::Align::Center)
                .menu_model(&menu)
                .build();
            more_button.add_css_class("flat");

            action_row.add_suffix(&insert_button);
            action_row.add_suffix(&more_button);

            let drag_source = gtk4::DragSource::new();
            drag_source.set_actions(gtk4::gdk::DragAction::MOVE);
            let drag_uuid = command.uuid.clone();
            drag_source.connect_prepare(move |_, _, _| {
                Some(gtk4::gdk::ContentProvider::for_value(&drag_uuid.to_value()))
            });
            action_row.add_controller(drag_source);

            let drop_target =
                gtk4::DropTarget::new(glib::Type::STRING, gtk4::gdk::DragAction::MOVE);
            let win = self.clone();
            let target_uuid = command.uuid.clone();
            drop_target.connect_drop(move |_, value, _, _| {
                if let Ok(source_uuid) = value.get::<String>()
                    && source_uuid != target_uuid
                {
                    win.reorder_command(&source_uuid, &target_uuid);
                    return true;
                }
                false
            });
            action_row.add_controller(drop_target);

            imp.command_list.append(&action_row);

            let win = self.clone();
            let command_for_run = command.clone();
            action_row.connect_activated(move |_| {
                win.execute_saved_command(&command_for_run, CommandRunMode::Run);
            });

            let win = self.clone();
            insert_button.connect_clicked(move |_| {
                win.execute_saved_command(&command, CommandRunMode::Insert);
            });
        }

        let is_empty = imp.command_list.row_at_index(0).is_none();
        imp.command_scroll.set_visible(!is_empty);
        imp.command_empty.set_visible(is_empty);
    }

    fn reorder_bookmark(&self, source_uuid: &str, target_uuid: &str) {
        let mut items = crate::bookmarks::load();
        crate::bookmarks::reorder(&mut items, source_uuid, target_uuid);
        let _ = crate::bookmarks::save(&items);
        self.refresh_bookmark_sidebar();
    }

    fn reorder_command(&self, source_uuid: &str, target_uuid: &str) {
        let mut items = commands::load();
        commands::reorder(&mut items, source_uuid, target_uuid);
        let _ = commands::save(&items);
        self.refresh_command_sidebar();
    }

    pub(crate) fn execute_saved_command(&self, command: &SavedCommand, run_mode: CommandRunMode) {
        let Some(terminal_uuid) = self.command_target_terminal_uuid() else {
            return;
        };

        self.set_terminal_recovery(
            &terminal_uuid,
            PaneRecovery {
                source: PaneSource::Command { title: command.title.clone() },
                target: None,
                startup: vec![StartupStep::SendText {
                    text: command.body.clone(),
                    execute: run_mode == CommandRunMode::Run,
                }],
            },
        );
        self.send_input_to_terminal(&terminal_uuid, &command.input_for(run_mode));
    }

    fn setup_bookmark_terminal(&self, terminal_uuid: &str, bookmark: &Bookmark) {
        let target = bookmark.pane_target();
        let startup = if target.is_none() {
            bookmark
                .session_startup_command()
                .as_deref()
                .map(|cmd| vec![StartupStep::SendText { text: cmd.to_string(), execute: true }])
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let recovery = PaneRecovery {
            source: PaneSource::Bookmark { name: bookmark.name.clone() },
            target,
            startup,
        };
        self.set_terminal_recovery(terminal_uuid, recovery.clone());
        if let Some(term) = self.imp().terminals.borrow().get(terminal_uuid).cloned() {
            self.attempt_recovery_for_terminal(&term, &recovery);
        }
    }

    fn trigger_managed_recovery_for_terminal(&self, terminal_uuid: &str) {
        let Some(recovery) = self.recovery_for_terminal(terminal_uuid) else {
            return;
        };

        if let Some(target) = recovery.target.as_ref()
            && let Some(startup_input) = target.managed_startup_input()
        {
            self.send_input_to_terminal(terminal_uuid, &startup_input);
            return;
        }

        for step in recovery.startup {
            self.send_input_to_terminal(terminal_uuid, &step.terminal_input());
        }
    }

    fn send_input_to_terminal(&self, terminal_uuid: &str, input: &str) {
        let sent = self.imp().terminals.borrow().get(terminal_uuid).cloned().map_or_else(
            || {
                if self.imp().persistent_terminals.borrow().contains_key(terminal_uuid) {
                    self.send_managed_terminal_input(terminal_uuid, input.as_bytes().to_vec());
                    true
                } else {
                    false
                }
            },
            |term| {
                term.queue_input_for_shell(input.to_string());
                true
            },
        );

        if sent
            && let Some(terminal) = self.terminal_handle(terminal_uuid)
            && terminal.grab_focus()
        {
            self.set_focused_terminal(Some(terminal_uuid));
        }
    }

    fn set_terminal_recovery(&self, terminal_uuid: &str, recovery: PaneRecovery) {
        let mut state = self.imp().state.borrow_mut();
        if let Some(session) = state
            .sessions
            .iter_mut()
            .find(|session| session.layout.contains_terminal(terminal_uuid))
        {
            session.set_recovery(terminal_uuid, recovery);
        }
    }

    fn recovery_for_terminal(&self, terminal_uuid: &str) -> Option<PaneRecovery> {
        let state = self.imp().state.borrow();
        state.sessions.iter().find_map(|session| {
            if session.layout.contains_terminal(terminal_uuid) {
                session.recovery_for(terminal_uuid).cloned()
            } else {
                None
            }
        })
    }

    fn attempt_recovery_for_terminal(&self, term: &TerminalWidget, recovery: &PaneRecovery) {
        term.hide_recovery_message();

        if let Some(target) = &recovery.target
            && let Some(startup_input) = target.managed_startup_input()
        {
            term.queue_input_for_shell(startup_input);
            return;
        }

        if recovery.startup.is_empty() {
            term.ensure_shell_spawned_when_ready();
            return;
        }
        for step in &recovery.startup {
            term.queue_input_for_shell(step.terminal_input());
        }
    }

    fn retry_terminal_recovery(&self, term: &TerminalWidget) {
        let uuid = term.uuid();
        let Some(recovery) = self.recovery_for_terminal(&uuid) else {
            return;
        };
        self.attempt_recovery_for_terminal(term, &recovery);
    }

    fn handle_recoverable_terminal_exit(
        &self,
        term: &TerminalWidget,
        terminal_uuid: &str,
        status: i32,
    ) -> bool {
        let Some(recovery) = self.recovery_for_terminal(terminal_uuid) else {
            return false;
        };
        let Some(target) = recovery.target else {
            return false;
        };
        if !target.manages_child_lifecycle() {
            return false;
        }

        term.reset_launch_state_for_retry();
        term.show_recovery_message(&target.failure_message(status));
        true
    }

    fn confirm_delete(&self, title: &str, body: &str, on_delete: impl Fn() + 'static) {
        let alert = adw::AlertDialog::new(Some(title), Some(body));
        alert.add_response("cancel", "Cancel");
        alert.add_response("delete", "Delete");
        alert.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        alert.set_default_response(Some("cancel"));
        alert.set_close_response("cancel");
        alert.connect_response(None, move |_, response| {
            if response == "delete" {
                on_delete();
            }
        });
        alert.present(Some(self));
    }

    fn confirm_close_session(&self, session_uuid: &str) {
        let should_close_immediately = {
            let state = self.imp().state.borrow();
            state.sessions.iter().find(|s| s.uuid == session_uuid).is_some_and(|session| {
                !session.uses_managed_runtime() && session.layout.terminal_count() <= 1
            })
        };
        if should_close_immediately {
            self.close_session(session_uuid);
            return;
        }

        let Some(presentation) = self.workspace_action_presentation(session_uuid) else {
            return;
        };

        let win = self.clone();
        let uuid = session_uuid.to_string();
        let alert = adw::AlertDialog::new(Some(&presentation.title), Some(&presentation.body));
        alert.add_response("cancel", "Cancel");
        alert.add_response("close", &presentation.close_label);
        alert.set_response_appearance("close", adw::ResponseAppearance::Destructive);
        alert.set_default_response(Some("cancel"));
        alert.set_close_response("cancel");
        alert.connect_response(None, move |_, response| {
            if response == "close" {
                win.close_session(&uuid);
            }
        });
        alert.present(Some(self));
    }

    fn confirm_delete_bookmark(&self, uuid: String) {
        let win = self.clone();
        self.confirm_delete(
            "Delete Bookmark?",
            "The bookmark will be permanently removed.",
            move || {
                let mut items = crate::bookmarks::load();
                items.retain(|b| b.uuid != uuid);
                if let Err(e) = crate::bookmarks::save(&items) {
                    log::error!("Failed to delete bookmark: {e}");
                }
                win.refresh_bookmark_sidebar();
            },
        );
    }

    fn confirm_delete_command(&self, uuid: String) {
        let win = self.clone();
        self.confirm_delete(
            "Delete Command?",
            "The command will be permanently removed.",
            move || {
                let mut items = commands::load();
                items.retain(|c| c.uuid != uuid);
                if let Err(e) = commands::save(&items) {
                    log::error!("Failed to delete command: {e}");
                }
                win.refresh_command_sidebar();
            },
        );
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

    fn split_terminal(&self, terminal_uuid: &str, orientation: SplitOrientation) {
        // Unzoom before splitting so the full layout is visible.
        {
            let state = self.imp().state.borrow();
            if let Some(session) =
                state.sessions.iter().find(|s| s.layout.contains_terminal(terminal_uuid))
                && session.is_zoomed()
            {
                drop(state);
                self.toggle_pane_zoom();
            }
        }

        let imp = self.imp();

        let source_cwd =
            self.terminal_handle(terminal_uuid).and_then(|terminal| terminal.current_directory());

        let mut state = imp.state.borrow_mut();

        let session_idx =
            state.sessions.iter().position(|s| s.layout.contains_terminal(terminal_uuid));

        if let Some(idx) = session_idx {
            let at_limit = state.sessions[idx]
                .layout
                .depth_of_terminal(terminal_uuid)
                .is_some_and(|d| d >= MAX_SPLIT_DEPTH);

            if at_limit {
                drop(state);
                self.show_toast("Maximum split depth reached");
                return;
            }

            if let Some((mut new_layout, new_terminal_uuid)) =
                state.sessions[idx].layout.split_terminal_with_new_uuid(terminal_uuid, orientation)
            {
                // Propagate the source terminal's CWD to the new terminal node.
                if let Some(cwd) = &source_cwd {
                    new_layout.set_terminal_cwd(&new_terminal_uuid, Some(cwd.clone()));
                }
                state.sessions[idx].layout = new_layout;
                state.sessions[idx].set_recovery(&new_terminal_uuid, PaneRecovery::empty_shell());
                let layout_terminal_uuids = state.sessions[idx].layout.terminal_uuids();
                state.sessions[idx].runtime.ensure_placeholder_bindings(&layout_terminal_uuids);
                state.sessions[idx].normalize_active_terminal();
                let session_uuid = state.sessions[idx].uuid.clone();
                let session_state = state.sessions[idx].clone();
                drop(state);
                if self.split_terminal_in_place(
                    &session_uuid,
                    terminal_uuid,
                    &new_terminal_uuid,
                    orientation,
                ) {
                    self.refresh_sidebar_subtitle(&session_uuid);
                } else {
                    self.rebuild_session_content(&session_uuid, &session_state);
                }

                if session_state.uses_managed_runtime()
                    && let Some(runtime_id) = session_state.runtime.runtime_id.as_deref()
                    && let Some(manager) = self.imp().connection_manager.borrow().as_ref()
                {
                    manager.create_pane(
                        &session_uuid,
                        &session_state.runtime.endpoint,
                        runtime_id,
                        &new_terminal_uuid,
                        source_cwd,
                        adw::StyleManager::default().is_dark(),
                    );
                }
            }
        }
    }

    fn close_terminal(&self, terminal_uuid: &str) {
        #[derive(Debug)]
        #[allow(clippy::large_enum_variant)]
        enum Action {
            CloseSession(String),
            Rebuild { session_uuid: String, session_state: SessionState },
        }

        // Unzoom before closing so the full layout is visible for removal.
        {
            let state = self.imp().state.borrow();
            if let Some(session) =
                state.sessions.iter().find(|s| s.layout.contains_terminal(terminal_uuid))
                && session.is_zoomed()
            {
                drop(state);
                self.toggle_pane_zoom();
            }
        }

        let imp = self.imp();

        let action = {
            let mut state = imp.state.borrow_mut();
            let session_idx =
                state.sessions.iter().position(|s| s.layout.contains_terminal(terminal_uuid));
            let Some(idx) = session_idx else { return };

            if state.sessions[idx].uses_managed_runtime()
                && state.sessions[idx].layout.terminal_count() > 1
                && let Some(runtime_id) = state.sessions[idx].runtime.runtime_id.clone()
                && let Some(runtime_pane_id) =
                    state.sessions[idx].runtime.pane_bindings.get(terminal_uuid).cloned()
                && runtime_pane_id != terminal_uuid
            {
                let workspace_id = state.sessions[idx].uuid.clone();
                let endpoint = state.sessions[idx].runtime.endpoint.clone();
                drop(state);
                if let Some(manager) = imp.connection_manager.borrow().as_ref() {
                    manager.close_pane(
                        &workspace_id,
                        &endpoint,
                        &runtime_id,
                        terminal_uuid,
                        &runtime_pane_id,
                    );
                }
                return;
            }

            if state.sessions[idx].layout.terminal_count() <= 1 {
                Action::CloseSession(state.sessions[idx].uuid.clone())
            } else if let Some(new_layout) =
                state.sessions[idx].layout.remove_terminal(terminal_uuid)
            {
                state.sessions[idx].layout = new_layout;
                let layout_terminal_uuids = state.sessions[idx].layout.terminal_uuids();
                state.sessions[idx].runtime.ensure_placeholder_bindings(&layout_terminal_uuids);
                state.sessions[idx].normalize_active_terminal();
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
        let previously_visible =
            imp.session_stack.visible_child_name().map(|name| name.to_string());

        let old_content = imp.session_stack.child_by_name(session_uuid);
        if let Some(ref old) = old_content {
            imp.session_stack.remove(old);
        }

        if let Some(ref old) = old_content {
            Self::detach_terminals_from_detached_tree(old);
        }
        drop(old_content);

        let content = self.build_session_content(session_state);

        imp.session_stack.add_named(&content, Some(session_uuid));
        let visible_after_rebuild = previously_visible
            .as_deref()
            .filter(|visible_uuid| *visible_uuid != session_uuid)
            .filter(|visible_uuid| imp.session_stack.child_by_name(visible_uuid).is_some())
            .unwrap_or(session_uuid);
        imp.session_stack.set_visible_child_name(visible_after_rebuild);
        if !session_state.is_zoomed() {
            session::schedule_initial_paned_ratios(&content, &session_state.layout);
        }

        self.refresh_sidebar_subtitle(session_uuid);
        self.sync_sidebar_to_visible_session();
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

        let pre_split_position = match orientation {
            SplitOrientation::Horizontal => target.width() / 2,
            SplitOrientation::Vertical => target.height() / 2,
        };

        let inherited_cwd = target.current_directory();
        let new_term = TerminalWidget::new(new_terminal_uuid, inherited_cwd.as_deref());
        self.connect_terminal_signals(&new_term);
        imp.terminals.borrow_mut().insert(new_terminal_uuid.to_string(), new_term.clone());
        // NOTE: shell spawn is deferred until after in-place surgery succeeds,
        // to avoid leaving a live PTY if the surgery fails (#2).

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
            if let Some(p) = branch.downcast_ref::<gtk4::Paned>() {
                let pos = pre_split_position;
                p.set_position(pos);
                p.connect_realize(move |paned| {
                    paned.set_position(pos);
                });
            }
            stack.add_named(&branch, Some(session_uuid));
            stack.set_visible_child_name(session_uuid);
            session::schedule_initial_paned_ratios(&branch, &branch_layout);
            if let Some(term) = imp.terminals.borrow().get(new_terminal_uuid) {
                term.ensure_shell_spawned_when_ready();
            }
            return true;
        }

        let Ok(paned) = parent.downcast::<gtk4::Paned>() else {
            Self::cleanup_unspliced_terminal(imp, new_terminal_uuid);
            return false;
        };

        let target_widget = target.upcast::<gtk4::Widget>();
        let start_child = paned.start_child();
        let end_child = paned.end_child();
        let is_start = start_child.as_ref() == Some(&target_widget);
        let is_end = end_child.as_ref() == Some(&target_widget);

        if !is_start && !is_end {
            Self::cleanup_unspliced_terminal(imp, new_terminal_uuid);
            return false;
        }

        if is_start {
            paned.set_start_child(None::<&gtk4::Widget>);
        } else {
            paned.set_end_child(None::<&gtk4::Widget>);
        }

        let branch = build_branch();
        if let Some(p) = branch.downcast_ref::<gtk4::Paned>() {
            let pos = pre_split_position;
            p.set_position(pos);
            p.connect_realize(move |paned| {
                paned.set_position(pos);
            });
        }
        if is_start {
            paned.set_start_child(Some(&branch));
        } else {
            paned.set_end_child(Some(&branch));
        }
        session::schedule_initial_paned_ratios(&branch, &branch_layout);
        if let Some(term) = imp.terminals.borrow().get(new_terminal_uuid) {
            term.ensure_shell_spawned_when_ready();
        }
        true
    }

    fn cleanup_unspliced_terminal(imp: &imp::Window, uuid: &str) {
        if let Some(term) = imp.terminals.borrow().get(uuid) {
            term.disconnect_child_exited();
        }
        imp.terminals.borrow_mut().remove(uuid);
    }

    fn refresh_sidebar_subtitle(&self, session_uuid: &str) {
        let imp = self.imp();
        let subtitle = {
            let state = imp.state.borrow();
            let Some(session) = state.sessions.iter().find(|s| s.uuid == session_uuid) else {
                return;
            };
            let endpoint = &session.runtime.endpoint;
            let active_uuid = session.active_terminal_uuid.as_deref();
            let pane_info = active_uuid.and_then(|uuid| {
                let handle = self.terminal_handle(uuid)?;
                pane_description(Some(&handle.title()), handle.current_directory().as_deref())
            });
            if session.uses_managed_runtime() {
                workspace_connection_summary(endpoint, pane_info.as_deref())
            } else {
                pane_info.unwrap_or_default()
            }
        };
        let mut idx = 0;
        while let Some(row) = imp.sidebar_list.row_at_index(idx) {
            if let Some(session_row) = row.child().and_then(|c| c.downcast::<SessionRow>().ok())
                && session_row.uuid() == session_uuid
            {
                session_row.set_subtitle(&subtitle);
                return;
            }
            idx += 1;
        }
    }

    fn refresh_sidebar_subtitle_if_active(&self, terminal_uuid: &str) {
        let session_uuid = {
            let state = self.imp().state.borrow();
            state
                .sessions
                .iter()
                .find(|s| s.active_terminal_uuid.as_deref() == Some(terminal_uuid))
                .map(|s| s.uuid.clone())
        };
        if let Some(session_uuid) = session_uuid {
            self.refresh_sidebar_subtitle(&session_uuid);
        }
    }

    fn set_input_sync(&self, enabled: bool) {
        let mut state = self.imp().state.borrow_mut();
        let active_idx = self.imp().sidebar_list.selected_row().map_or(0, |r| r.index() as usize);
        if let Some(session) = state.sessions.get_mut(active_idx) {
            session.input_sync = enabled;
        }
    }

    fn apply_preferences_to_terminal(
        term: &TerminalWidget,
        prefs: &Preferences,
        font_desc: &gtk4::pango::FontDescription,
        scheme: Option<&color_scheme::ColorScheme>,
    ) {
        let vte = term.vte();
        vte.set_font(Some(font_desc));
        vte.set_scrollback_lines(prefs.scrollback_lines);
        vte.set_scroll_on_keystroke(prefs.scroll_on_keystroke);
        vte.set_scroll_on_output(prefs.scroll_on_output);
        vte.set_audible_bell(prefs.audible_bell);
        term.set_visual_bell(prefs.visual_bell);
        term.set_smart_clipboard(prefs.smart_clipboard);
        term.imp().header.set_visible(prefs.show_headerbar);
        if let Some(scheme) = scheme {
            term.apply_color_scheme(scheme);
        }
    }

    pub(crate) fn reapply_terminal_preferences(&self) {
        let prefs = preferences::load();

        // Update pane navigation shortcuts in case the keybinding changed.
        if let Some(app) = self.application().and_downcast::<adw::Application>() {
            let (left, right, up, down) = prefs.pane_navigation_keys.accels();
            for (name, accel) in [
                ("navigate-left", left),
                ("navigate-right", right),
                ("navigate-up", up),
                ("navigate-down", down),
            ] {
                app.set_accels_for_action(&format!("win.{name}"), &[accel]);
            }
        }

        let font_desc = gtk4::pango::FontDescription::from_string(&prefs.font);
        let is_dark = adw::StyleManager::default().is_dark();
        let effective_name = prefs.effective_color_scheme_name(is_dark);
        let scheme = color_scheme::load_color_scheme_by_name(effective_name).or_else(|| {
            let fallback = if is_dark {
                color_scheme::BUILTIN_DARK_SCHEME_NAME
            } else {
                color_scheme::BUILTIN_LIGHT_SCHEME_NAME
            };
            color_scheme::load_color_scheme_by_name(fallback)
        });
        let terminals: Vec<TerminalWidget> =
            self.imp().terminals.borrow().values().cloned().collect();
        for term in terminals {
            Self::apply_preferences_to_terminal(&term, &prefs, &font_desc, scheme.as_ref());
        }
        let persistent: Vec<PersistentPaneView> =
            self.imp().persistent_terminals.borrow().values().cloned().collect();
        for pane in persistent {
            self.apply_preferences_to_persistent_pane(&pane);
        }
    }

    fn apply_preferences_to_persistent_pane(&self, pane: &PersistentPaneView) {
        let prefs = preferences::load();
        let font_desc = gtk4::pango::FontDescription::from_string(&prefs.font);
        pane.vte().set_font(Some(&font_desc));
        pane.vte().set_scrollback_lines(prefs.scrollback_lines);
        pane.set_smart_clipboard(prefs.smart_clipboard);

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
            pane.apply_color_scheme(&scheme);
        }
    }

    fn forward_input(&self, source_uuid: &str, text: &str) {
        let state = self.imp().state.borrow();
        let session =
            state.sessions.iter().find(|s| s.input_sync && s.layout.contains_terminal(source_uuid));
        let Some(session) = session else { return };
        let uuids = session.layout.terminal_uuids();
        drop(state);

        let terminals = self.imp().terminals.borrow();
        for uuid in &uuids {
            if uuid != source_uuid
                && let Some(term) = terminals.get(uuid)
            {
                term.vte().feed_child(text.as_bytes());
            }
        }
    }

    fn focused_terminal_uuid(&self) -> Option<String> {
        self.imp().focused_terminal_uuid.borrow().clone()
    }

    fn terminal_handle(&self, terminal_uuid: &str) -> Option<TerminalHandle> {
        if let Some(term) = self.imp().terminals.borrow().get(terminal_uuid).cloned() {
            return Some(TerminalHandle::Direct(term));
        }
        self.imp()
            .persistent_terminals
            .borrow()
            .get(terminal_uuid)
            .cloned()
            .map(TerminalHandle::Managed)
    }

    fn set_focused_terminal(&self, terminal_uuid: Option<&str>) {
        let next = terminal_uuid.map(str::to_string);
        let previous = self.imp().focused_terminal_uuid.replace(next.clone());
        let previous_terminal = previous.as_deref().and_then(|uuid| self.terminal_handle(uuid));
        let next_terminal = next.as_deref().and_then(|uuid| self.terminal_handle(uuid));

        if let Some(terminal) = previous_terminal
            && previous != next
        {
            terminal.set_active(false);
        }
        if let Some(terminal) = next_terminal {
            terminal.set_active(true);
        }
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

    fn navigate_focused(&self, direction: Direction) {
        let Some(current_uuid) = self.focused_terminal_uuid() else { return };
        let adjacent_uuid = {
            let state = self.imp().state.borrow();
            state
                .sessions
                .iter()
                .find(|s| s.layout.contains_terminal(&current_uuid))
                .and_then(|s| s.layout.find_adjacent(&current_uuid, direction))
        };
        if let Some(target_uuid) = adjacent_uuid
            && let Some(terminal) = self.terminal_handle(&target_uuid)
        {
            let win = self.clone();
            glib::idle_add_local_once(move || {
                if terminal.grab_focus() {
                    win.set_focused_terminal(Some(&target_uuid));
                }
            });
        }
    }

    pub(crate) fn show_toast(&self, message: &str) {
        self.imp().toast_overlay.add_toast(adw::Toast::new(message));
    }

    fn toggle_focused_search(&self) {
        if let Some(uuid) = self.focused_terminal_uuid()
            && let Some(terminal) = self.terminal_handle(&uuid)
        {
            terminal.toggle_search();
        }
    }

    fn cycle_session(&self, delta: i32) {
        let imp = self.imp();
        let state = imp.state.borrow();
        let count = state.sessions.len() as i32;
        if count <= 1 {
            return;
        }
        let current = imp.sidebar_list.selected_row().map_or(0, |r| r.index());
        let next = (current + delta).rem_euclid(count);
        drop(state);
        if let Some(row) = imp.sidebar_list.row_at_index(next) {
            imp.sidebar_list.select_row(Some(&row));
        }
    }

    fn switch_to_session_number(&self, number: usize) {
        let Some(index) = number.checked_sub(1) else {
            return;
        };

        if let Some(row) = self.imp().sidebar_list.row_at_index(index as i32) {
            self.imp().sidebar_list.select_row(Some(&row));
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
        if let Some(uuid) = self.focused_terminal_uuid()
            && let Some(terminal) = self.terminal_handle(&uuid)
        {
            terminal.zoom(direction);
        }
    }

    fn toggle_pane_zoom(&self) {
        let Some(session_uuid) =
            self.imp().session_stack.visible_child_name().map(|n| n.to_string())
        else {
            return;
        };
        let session_state = {
            let mut state = self.imp().state.borrow_mut();
            let Some(session) = state.sessions.iter_mut().find(|s| s.uuid == session_uuid) else {
                return;
            };
            if session.is_zoomed() {
                session.zoomed_terminal_uuid = None;
            } else {
                let Some(focused) = self.focused_terminal_uuid() else {
                    return;
                };
                if session.layout.terminal_count() < 2 {
                    return;
                }
                session.zoomed_terminal_uuid = Some(focused);
            }
            session.clone()
        };
        self.rebuild_session_content(&session_uuid, &session_state);
        self.focus_session_terminal(&session_uuid);
    }

    fn mark_session_activity(&self, terminal_uuid: &str) {
        let imp = self.imp();
        let visible_session = imp.session_stack.visible_child_name();
        let state = imp.state.borrow();
        if !terminal_is_in_background_session(terminal_uuid, visible_session.as_deref(), &state) {
            return;
        }
        let session_uuid = state
            .sessions
            .iter()
            .find(|s| s.layout.contains_terminal(terminal_uuid))
            .map(|s| s.uuid.clone());
        drop(state);
        let Some(session_uuid) = session_uuid else { return };
        let list = &imp.sidebar_list;
        let mut idx = 0;
        while let Some(row) = list.row_at_index(idx) {
            if let Some(session_row) = row.child().and_then(|c| c.downcast::<SessionRow>().ok())
                && session_row.uuid() == session_uuid
            {
                session_row.mark_activity();
                break;
            }
            idx += 1;
        }
    }

    fn notify_process_completed(&self, terminal_uuid: &str, status: i32) {
        let title = self
            .terminal_handle(terminal_uuid)
            .map_or_else(|| "Terminal".into(), |terminal| terminal.title());

        let body = if status == 0 {
            format!("\"{title}\" completed successfully")
        } else {
            format!("\"{title}\" exited with status {status}")
        };

        let tier = {
            let visible = self.imp().session_stack.visible_child_name();
            let state = self.imp().state.borrow();
            notification_tier(terminal_uuid, visible.as_deref(), self.is_active(), &state)
        };

        match tier {
            NotificationTier::Suppress => {}
            NotificationTier::Toast => self.show_toast(&body),
            NotificationTier::Desktop => {
                let notification = gtk4::gio::Notification::new("Process completed");
                notification.set_body(Some(&body));
                if let Some(app) = self.application() {
                    app.send_notification(None, &notification);
                }
            }
        }
    }

    pub(crate) fn create_bookmark_from_active_session(&self) -> Option<Bookmark> {
        let uuid = self.focused_terminal_uuid()?;
        let state = self.imp().state.borrow();
        let session = state.sessions.iter().find(|s| s.layout.contains_terminal(&uuid))?;
        let session_name = session.name.clone();
        drop(state);

        let cwd = self.terminal_handle(&uuid).and_then(|terminal| terminal.current_directory());

        let mut bookmark = Bookmark::new(session_name);
        bookmark.directory = cwd;
        Some(bookmark)
    }

    fn do_bookmark_active_session(&self) {
        let Some(bookmark) = self.create_bookmark_from_active_session() else {
            return;
        };
        let name = bookmark.name.clone();
        let mut bookmarks = crate::bookmarks::load();
        bookmarks.push(bookmark);
        let _ = crate::bookmarks::save(&bookmarks);
        self.refresh_bookmark_sidebar();

        let notification = gtk4::gio::Notification::new("Bookmark saved");
        notification.set_body(Some(&format!("Workspace \"{name}\" was added to bookmarks")));
        if let Some(app) = self.application() {
            app.send_notification(None, &notification);
        }
    }

    fn clipboard_copy(&self) {
        if let Some(uuid) = self.focused_terminal_uuid()
            && let Some(terminal) = self.terminal_handle(&uuid)
        {
            terminal.copy_clipboard();
        }
    }

    fn clipboard_paste(&self) {
        if let Some(uuid) = self.focused_terminal_uuid()
            && let Some(terminal) = self.terminal_handle(&uuid)
        {
            match terminal {
                TerminalHandle::Direct(terminal) => terminal.vte().paste_clipboard(),
                TerminalHandle::Managed(pane) => {
                    let win = self.clone();
                    let terminal_uuid = uuid.clone();
                    pane.request_clipboard_paste(move |bytes| {
                        win.send_managed_terminal_input(&terminal_uuid, bytes);
                    });
                }
            }
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
