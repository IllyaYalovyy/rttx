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
use crate::preferences;
use crate::session::{
    self, LayoutNode, MAX_SPLIT_DEPTH, PaneRecovery, PaneSource, PaneTarget, SessionState,
    SplitOrientation, StartupStep, WindowState,
};
use crate::sidebar::SessionRow;
use crate::terminal::widget::TerminalWidget;
use std::collections::HashMap;

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
            self.add_session_button.set_tooltip_text(Some("New session"));
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
            header.pack_end(&toggle_utility_sidebar);

            let menu_button = gtk4::MenuButton::new();
            menu_button.set_icon_name("open-menu-symbolic");

            let menu = gtk4::gio::Menu::new();
            menu.append(Some("About rttx"), Some("win.about"));
            menu.append(Some("Bookmark This Session"), Some("win.bookmark-session"));
            menu.append(Some("Preferences"), Some("win.preferences"));
            menu.append(Some("Sync Input"), Some("win.toggle-input-sync"));
            menu.append(Some("Keyboard Shortcuts"), Some("win.show-help-overlay"));
            menu.append(Some("Fullscreen"), Some("win.fullscreen"));
            menu_button.set_menu_model(Some(&menu));

            header.pack_end(&menu_button);

            self.sidebar_list.set_selection_mode(gtk4::SelectionMode::Single);
            self.sidebar_list.add_css_class("navigation-sidebar");
            self.sidebar_list.update_property(&[gtk4::accessible::Property::Label("Sessions")]);
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
            self.command_scroll.set_child(Some(&self.command_list));
            self.command_scroll.set_visible(false);

            self.command_empty.set_icon_name(Some("system-run-symbolic"));
            self.command_empty.set_title("No Commands");
            self.command_empty.set_description(Some(
                "Save frequently used commands to run or insert from the sidebar",
            ));
            self.command_empty.set_vexpand(true);

            let templates_placeholder = gtk4::Label::new(Some("Session templates will live here."));
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
        let active_index = state.active_session_index;
        let is_maximized = state.is_maximized;
        let width = state.width;
        let height = state.height;
        let left_sidebar_width = state.left_sidebar_width;
        let right_sidebar_width = state.right_sidebar_width;

        self.imp().state.replace(state.clone());

        for session in &state.sessions {
            self.build_session(session);
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
            if let Some(content) = imp.session_stack.child_by_name(&session.uuid) {
                session::capture_paned_ratios(&mut session.layout, &content);
            }
            session.prune_recovery();
            session.normalize_active_terminal();
        }

        {
            let terminals = imp.terminals.borrow();
            for session in &mut state.sessions {
                for node_uuid in session.layout.terminal_uuids() {
                    if let Some(term) = terminals.get(&node_uuid) {
                        session.layout.set_terminal_cwd(&node_uuid, term.current_directory());
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
            ("new-session", &["<Ctrl><Shift>T"], Self::add_session),
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

        let list_row = gtk4::ListBoxRow::new();
        list_row.set_child(Some(&row));
        imp.sidebar_list.append(&list_row);

        let content = self.build_session_content(session_state);

        imp.session_stack.add_named(&content, Some(&session_state.uuid));
        session::schedule_initial_paned_ratios(&content, &session_state.layout);
        self.update_sidebar_count(&session_state.uuid, session_state.layout.terminal_count());
    }

    fn build_session_content(&self, session_state: &SessionState) -> gtk4::Widget {
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
            term.set_title(title);
            term.imp().custom_title.replace(Some(title.to_string()));
        }
        self.connect_terminal_signals(&term);
        self.imp().terminals.borrow_mut().insert(uuid.to_string(), term.clone());
        self.initialize_terminal_recovery(&term, session_state, uuid);
        term.upcast()
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
        popover.set_parent(row);

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
        self.apply_preferences_to_terminal(term);

        let win = self.clone();
        let uuid = term.uuid();
        let focus_controller = gtk4::EventControllerFocus::new();
        focus_controller.connect_enter(move |_| {
            win.imp().focused_terminal_uuid.replace(Some(uuid.clone()));
            let mut state = win.imp().state.borrow_mut();
            if let Some(session) =
                state.sessions.iter_mut().find(|session| session.layout.contains_terminal(&uuid))
            {
                session.active_terminal_uuid = Some(uuid.clone());
            }
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
        about.set_version(env!("CARGO_PKG_VERSION"));
        about.set_developer_name(config::DEVELOPER_NAME);
        about.set_developers(&[config::DEVELOPER_NAME]);
        about.set_website(config::PROJECT_WEBSITE);
        about.set_issue_url(config::ISSUE_TRACKER);
        about.set_license_type(gtk4::License::Gpl30);
        about.present();
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

    pub(crate) fn new_session_from_bookmark(&self, bookmark: &Bookmark) {
        let imp = self.imp();
        let initial_cwd = bookmark
            .pane_target()
            .as_ref()
            .and_then(PaneTarget::initial_cwd)
            .map(str::to_string)
            .or_else(|| bookmark.session_initial_cwd().map(str::to_string));

        let session_state = SessionState::new_with_initial_cwd(bookmark.name.clone(), initial_cwd);
        let session_uuid = session_state.uuid.clone();
        let terminal_uuid = session_state.layout.terminal_uuids().into_iter().next().unwrap();
        imp.state.borrow_mut().sessions.push(session_state.clone());
        self.build_session(&session_state);

        let index = imp.state.borrow().sessions.len() as i32 - 1;
        if let Some(row) = imp.sidebar_list.row_at_index(index) {
            imp.sidebar_list.select_row(Some(&row));
        }

        self.setup_bookmark_terminal(&terminal_uuid, bookmark);
        self.imp().session_stack.set_visible_child_name(&session_uuid);
    }

    pub(crate) fn execute_bookmark(&self, bookmark: &Bookmark) {
        let Some(term) = self.command_target_terminal() else {
            return;
        };

        let Some(command) = bookmark.command() else {
            return;
        };
        self.set_terminal_recovery(
            &term.uuid(),
            PaneRecovery {
                source: PaneSource::Bookmark { name: bookmark.name.clone() },
                target: None,
                startup: vec![StartupStep::SendText { text: command.clone(), execute: true }],
            },
        );
        self.send_input_to_terminal(&term.uuid(), &format!("{command}\n"));
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

            self.imp().terminals.borrow().get(&preferred_uuid).cloned()
        };

        let Some(term) = target else {
            return;
        };
        let win = self.clone();
        let target_uuid = term.uuid();
        glib::idle_add_local_once(move || {
            if term.vte().grab_focus() {
                win.imp().focused_terminal_uuid.replace(Some(target_uuid));
            }
        });
    }

    fn command_target_terminal(&self) -> Option<TerminalWidget> {
        let visible_session_uuid =
            self.imp().session_stack.visible_child_name().map(|name| name.to_string());
        let target_uuid = {
            let state = self.imp().state.borrow();
            preferred_command_target_uuid(
                self.focused_terminal_uuid().as_deref(),
                visible_session_uuid.as_deref(),
                &state,
            )
        }?;

        self.imp().terminals.borrow().get(&target_uuid).cloned()
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
            let row = gtk4::ListBoxRow::new();
            let action_row = adw::ActionRow::new();
            action_row.set_title(&bookmark.name);
            action_row.set_subtitle(&bookmark.summary());

            let run_button = gtk4::Button::builder()
                .icon_name("go-next-symbolic")
                .tooltip_text("Run in current pane")
                .valign(gtk4::Align::Center)
                .build();
            let new_session_button = gtk4::Button::builder()
                .icon_name("window-new-symbolic")
                .tooltip_text("New session from bookmark")
                .valign(gtk4::Align::Center)
                .build();

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

            action_row.add_suffix(&run_button);
            action_row.add_suffix(&new_session_button);
            action_row.add_suffix(&more_button);
            row.set_child(Some(&action_row));
            imp.bookmark_list.append(&row);

            let win = self.clone();
            let bookmark_for_run = bookmark.clone();
            run_button.connect_clicked(move |_| {
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
            let row = gtk4::ListBoxRow::new();
            let action_row = adw::ActionRow::new();
            action_row.set_title(&command.title);
            action_row.set_subtitle(&command.preview());

            let run_button = gtk4::Button::builder()
                .icon_name("go-next-symbolic")
                .tooltip_text("Run in current pane")
                .valign(gtk4::Align::Center)
                .build();
            let insert_button = gtk4::Button::builder()
                .icon_name("insert-text-symbolic")
                .tooltip_text("Insert into current pane")
                .valign(gtk4::Align::Center)
                .build();

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

            action_row.add_suffix(&run_button);
            action_row.add_suffix(&insert_button);
            action_row.add_suffix(&more_button);
            row.set_child(Some(&action_row));
            imp.command_list.append(&row);

            let win = self.clone();
            let command_for_run = command.clone();
            run_button.connect_clicked(move |_| {
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

    pub(crate) fn execute_saved_command(&self, command: &SavedCommand, run_mode: CommandRunMode) {
        let Some(term) = self.command_target_terminal() else {
            return;
        };

        self.set_terminal_recovery(
            &term.uuid(),
            PaneRecovery {
                source: PaneSource::Command { title: command.title.clone() },
                target: None,
                startup: vec![StartupStep::SendText {
                    text: command.body.clone(),
                    execute: run_mode == CommandRunMode::Run,
                }],
            },
        );
        self.send_input_to_terminal(&term.uuid(), &command.input_for(run_mode));
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

    fn send_input_to_terminal(&self, terminal_uuid: &str, input: &str) {
        let Some(term) = self.imp().terminals.borrow().get(terminal_uuid).cloned() else {
            return;
        };
        term.queue_input_for_shell(input.to_string());
        let _ = term.vte().grab_focus();
        self.imp().focused_terminal_uuid.replace(Some(term.uuid()));
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
    }

    fn split_terminal(&self, terminal_uuid: &str, orientation: SplitOrientation) {
        let imp = self.imp();
        let mut state = imp.state.borrow_mut();

        let session_idx = state
            .sessions
            .iter()
            .position(|s| s.layout.terminal_uuids().contains(&terminal_uuid.to_string()));

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

            if let Some((new_layout, new_terminal_uuid)) =
                state.sessions[idx].layout.split_terminal_with_new_uuid(terminal_uuid, orientation)
            {
                state.sessions[idx].layout = new_layout;
                state.sessions[idx].set_recovery(&new_terminal_uuid, PaneRecovery::empty_shell());
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
        imp.session_stack.set_visible_child_name(session_uuid);
        session::schedule_initial_paned_ratios(&content, &session_state.layout);

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
            session::schedule_initial_paned_ratios(&branch, &branch_layout);
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
        session::schedule_initial_paned_ratios(&branch, &branch_layout);
        true
    }

    fn update_sidebar_count(&self, session_uuid: &str, count: usize) {
        let imp = self.imp();
        let mut idx = 0;
        while let Some(row) = imp.sidebar_list.row_at_index(idx) {
            if let Some(session_row) = row.child().and_then(|c| c.downcast::<SessionRow>().ok())
                && session_row.uuid() == session_uuid
            {
                session_row.update_terminal_count(count);
                return;
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
        term.set_smart_clipboard(prefs.smart_clipboard);

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

    fn show_toast(&self, message: &str) {
        self.imp().toast_overlay.add_toast(adw::Toast::new(message));
    }

    fn toggle_focused_search(&self) {
        if let Some(uuid) = self.focused_terminal_uuid()
            && let Some(term) = self.imp().terminals.borrow().get(&uuid)
        {
            term.toggle_search();
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
            && let Some(term) = self.imp().terminals.borrow().get(&uuid)
        {
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

    pub(crate) fn create_bookmark_from_active_session(&self) -> Option<Bookmark> {
        let uuid = self.focused_terminal_uuid()?;
        let state = self.imp().state.borrow();
        let session = state.sessions.iter().find(|s| s.layout.contains_terminal(&uuid))?;
        let session_name = session.name.clone();
        drop(state);

        let cwd =
            self.imp().terminals.borrow().get(&uuid).and_then(TerminalWidget::current_directory);

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
        notification.set_body(Some(&format!("Session \"{name}\" was added to bookmarks")));
        if let Some(app) = self.application() {
            app.send_notification(None, &notification);
        }
    }

    fn clipboard_copy(&self) {
        if let Some(uuid) = self.focused_terminal_uuid()
            && let Some(term) = self.imp().terminals.borrow().get(&uuid)
        {
            term.vte().copy_clipboard_format(vte4::Format::Text);
        }
    }

    fn clipboard_paste(&self) {
        if let Some(uuid) = self.focused_terminal_uuid()
            && let Some(term) = self.imp().terminals.borrow().get(&uuid)
        {
            term.vte().paste_clipboard();
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
mod tests {
    use super::*;
    use std::sync::Once;
    use std::time::{Duration, Instant};

    static GTK_INIT: Once = Once::new();

    fn ensure_gtk_init() -> bool {
        let mut success = false;
        GTK_INIT.call_once(|| {
            crate::test_helpers::set_env("GTK_A11Y", "none");
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
                },
                SessionState {
                    uuid: "s2".into(),
                    name: "Session 2".into(),
                    layout: LayoutNode::new_terminal_with_uuid("t2"),
                    terminal_recovery: Default::default(),
                    active_terminal_uuid: None,
                    input_sync: false,
                },
            ],
            ..WindowState::default()
        }
    }

    #[test]
    fn terminal_in_background_session_triggers_notification() {
        let state = make_state_two_sessions();
        assert!(
            terminal_is_in_background_session("t1", Some("s2"), &state),
            "t1 is in s1 which is not visible (s2 is) — should notify"
        );
    }

    #[test]
    fn terminal_in_visible_session_suppresses_notification() {
        let state = make_state_two_sessions();
        assert!(
            !terminal_is_in_background_session("t1", Some("s1"), &state),
            "t1 is in s1 which IS visible — should not notify"
        );
    }

    #[test]
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
            }],
            ..WindowState::default()
        };
        assert!(
            !terminal_is_in_background_session("t2", Some("s1"), &state),
            "t2 is in the visible session s1 even though it is not focused — should not notify"
        );
    }

    #[test]
    fn terminal_is_background_when_no_visible_session() {
        let state = make_state_two_sessions();
        assert!(
            terminal_is_in_background_session("t1", None, &state),
            "when no session is visible, treat terminal as background"
        );
    }

    #[test]
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
            }],
            ..WindowState::default()
        };

        assert_eq!(
            preferred_command_target_uuid(Some("t2"), Some("s1"), &state).as_deref(),
            Some("t2")
        );
    }

    #[test]
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
                },
            ],
            ..WindowState::default()
        };

        assert_eq!(preferred_command_target_uuid(None, Some("s2"), &state).as_deref(), Some("t2"));
    }

    #[test]
    fn add_session_button_has_plus_icon() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
        crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

        let app = adw::Application::builder()
            .application_id("com.illya.rttx.add-session-icon-tests")
            .build();
        app.register(gtk4::gio::Cancellable::NONE).unwrap();

        let window = Window::new(&app);

        assert_eq!(
            window.imp().add_session_button.icon_name().as_deref(),
            Some("list-add-symbolic"),
            "new session button should expose the plus icon"
        );

        window.close();
    }

    #[test]
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
    fn utility_sidebar_shows_and_filters_bookmarks() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
        crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

        let mut local = crate::bookmarks::Bookmark::new("Local Project");
        local.directory = Some("/home/user/Projects/rttx".into());
        let mut remote = crate::bookmarks::Bookmark::new("Prod Web");
        remote.ssh_target = Some("deploy@example.com".into());
        crate::bookmarks::save(&[local, remote]).unwrap();

        let app = adw::Application::builder()
            .application_id("com.illya.rttx.utility-sidebar-tests")
            .build();
        app.register(gtk4::gio::Cancellable::NONE).unwrap();

        let window = Window::new(&app);
        window.present();
        pump_events(100);

        assert_eq!(
            window.imp().bookmark_list.observe_children().n_items(),
            2,
            "utility sidebar should show saved bookmarks"
        );

        window.imp().bookmark_search_entry.set_text("prod");
        pump_events(50);
        assert_eq!(
            window.imp().bookmark_list.observe_children().n_items(),
            1,
            "search should filter the utility sidebar bookmark list"
        );

        window.close();
        crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
    }

    #[test]
    fn utility_sidebar_shows_and_filters_commands() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
        crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

        let run = crate::commands::SavedCommand::new("Restart app", "systemctl restart app");
        let insert =
            crate::commands::SavedCommand::new("Deploy checklist", "cargo build\ncargo test");
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
    fn new_session_from_bookmark_creates_and_focuses_named_session() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
        crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

        let app = adw::Application::builder()
            .application_id("com.illya.rttx.bookmark-session-tests")
            .build();
        app.register(gtk4::gio::Cancellable::NONE).unwrap();

        let window = Window::new(&app);
        window.set_default_size(900, 600);
        window.present();
        pump_events(100);

        let mut bookmark = crate::bookmarks::Bookmark::new("Prod Web");
        bookmark.ssh_target = Some("deploy@example.com".into());
        bookmark.tmux_session = Some("web".into());

        window.new_session_from_bookmark(&bookmark);
        pump_events(100);

        let (session_name, session_uuid, terminal_uuid) = {
            let state = window.imp().state.borrow();
            assert_eq!(state.sessions.len(), 2, "bookmark should create a new session");
            let session = state.sessions.last().unwrap();
            (
                session.name.clone(),
                session.uuid.clone(),
                session.layout.terminal_uuids().into_iter().next().unwrap(),
            )
        };

        assert_eq!(session_name, "Prod Web");
        assert_eq!(
            window.imp().sidebar_list.selected_row().map(|row| row.index()),
            Some(1),
            "bookmark session should become the selected session"
        );
        assert_eq!(
            window.imp().session_stack.visible_child_name().as_deref(),
            Some(session_uuid.as_str()),
            "bookmark session should become visible"
        );
        assert_eq!(
            window.focused_terminal_uuid().as_deref(),
            Some(terminal_uuid.as_str()),
            "bookmark session should focus its initial terminal"
        );

        window.close();
        crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
    }

    #[test]
    fn new_session_from_bookmark_queues_input_before_shell_starts() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
        crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

        let app = adw::Application::builder()
            .application_id("com.illya.rttx.bookmark-queue-tests")
            .build();
        app.register(gtk4::gio::Cancellable::NONE).unwrap();

        let window = Window::new(&app);
        let mut bookmark = crate::bookmarks::Bookmark::new("Prod Web");
        bookmark.ssh_target = Some("deploy@example.com".into());
        bookmark.tmux_session = Some("web".into());
        let expected_input = PaneTarget::RemoteTmux {
            ssh_target: "deploy@example.com".into(),
            tmux_session: "web".into(),
        }
        .managed_startup_input()
        .unwrap();

        window.new_session_from_bookmark(&bookmark);

        let terminal_uuid = {
            let state = window.imp().state.borrow();
            state.sessions.last().unwrap().layout.terminal_uuids().into_iter().next().unwrap()
        };
        let term = window
            .imp()
            .terminals
            .borrow()
            .get(&terminal_uuid)
            .cloned()
            .expect("bookmark session terminal should exist");

        assert!(
            !term.shell_spawned_for_test(),
            "shell should not start before the window is presented"
        );
        assert_eq!(
            term.pending_shell_inputs_for_test(),
            vec![expected_input],
            "bookmark launcher should queue the structured recovery target until the shell is ready"
        );

        window.close();
        crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
    }

    #[test]
    fn bookmark_sessions_persist_and_replay_recovery_recipe_on_restart() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
        crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

        let app = adw::Application::builder()
            .application_id("com.illya.rttx.bookmark-recovery-tests")
            .build();
        app.register(gtk4::gio::Cancellable::NONE).unwrap();

        let first_window = Window::new(&app);
        let mut bookmark = crate::bookmarks::Bookmark::new("Prod Web");
        bookmark.ssh_target = Some("deploy@example.com".into());
        bookmark.tmux_session = Some("web".into());
        let expected_input = PaneTarget::RemoteTmux {
            ssh_target: "deploy@example.com".into(),
            tmux_session: "web".into(),
        }
        .managed_startup_input()
        .unwrap();

        first_window.new_session_from_bookmark(&bookmark);

        let (terminal_uuid, saved_recovery) = {
            let state = first_window.imp().state.borrow();
            let session = state.sessions.last().expect("bookmark should create a session");
            let terminal_uuid = session.layout.terminal_uuids().into_iter().next().unwrap();
            (terminal_uuid.clone(), session.recovery_for(&terminal_uuid).cloned())
        };

        assert_eq!(
            saved_recovery,
            Some(PaneRecovery {
                source: PaneSource::Bookmark { name: "Prod Web".into() },
                target: Some(PaneTarget::RemoteTmux {
                    ssh_target: "deploy@example.com".into(),
                    tmux_session: "web".into(),
                }),
                startup: vec![],
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
            .expect("restored bookmark terminal should exist");

        assert_eq!(
            restored_term.pending_shell_inputs_for_test(),
            vec![expected_input],
            "restored bookmark session should queue its structured recovery target before shell startup"
        );

        second_window.close();
        crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
    }

    #[test]
    fn new_session_from_folder_bookmark_uses_initial_cwd_not_cd_command() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
        crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

        let app = adw::Application::builder()
            .application_id("com.illya.rttx.folder-bookmark-cwd-tests")
            .build();
        app.register(gtk4::gio::Cancellable::NONE).unwrap();

        let window = Window::new(&app);
        let mut bookmark = crate::bookmarks::Bookmark::new("Work");
        bookmark.directory = Some("/home/user/work".into());

        window.new_session_from_bookmark(&bookmark);

        let (layout_cwd, pending_inputs) = {
            let state = window.imp().state.borrow();
            let session = state.sessions.last().unwrap();
            let terminal_uuid = session.layout.terminal_uuids().into_iter().next().unwrap();
            let layout_cwd = match &session.layout {
                LayoutNode::Terminal { cwd, .. } => cwd.clone(),
                _ => None,
            };
            let term = window.imp().terminals.borrow().get(&terminal_uuid).cloned().unwrap();
            (layout_cwd, term.pending_shell_inputs_for_test())
        };

        assert_eq!(
            layout_cwd.as_deref(),
            Some("/home/user/work"),
            "folder bookmark should set cwd in the layout node, not send a cd command"
        );
        assert!(
            pending_inputs.is_empty(),
            "folder-only bookmark should not queue any shell input; got: {pending_inputs:?}"
        );

        window.close();
        crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
    }

    #[test]
    fn new_session_from_ssh_bookmark_queues_ssh_command() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
        crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

        let app =
            adw::Application::builder().application_id("com.illya.rttx.ssh-bookmark-tests").build();
        app.register(gtk4::gio::Cancellable::NONE).unwrap();

        let window = Window::new(&app);
        let mut bookmark = crate::bookmarks::Bookmark::new("Prod");
        bookmark.ssh_target = Some("deploy@example.com".into());

        window.new_session_from_bookmark(&bookmark);

        let (pending_inputs, saved_recovery) = {
            let state = window.imp().state.borrow();
            let session = state.sessions.last().unwrap();
            let terminal_uuid = session.layout.terminal_uuids().into_iter().next().unwrap();
            let term = window.imp().terminals.borrow().get(&terminal_uuid).cloned().unwrap();
            (term.pending_shell_inputs_for_test(), session.recovery_for(&terminal_uuid).cloned())
        };

        assert_eq!(
            pending_inputs,
            vec!["exec ssh deploy@example.com\n"],
            "SSH bookmark should queue the structured ssh recovery command"
        );
        assert_eq!(
            saved_recovery,
            Some(PaneRecovery {
                source: PaneSource::Bookmark { name: "Prod".into() },
                target: Some(PaneTarget::RemoteShell {
                    ssh_target: "deploy@example.com".into(),
                    remote_folder: None,
                }),
                startup: vec![],
            })
        );

        window.close();
        crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
    }

    #[test]
    fn new_session_from_local_dir_and_tmux_bookmark_uses_initial_cwd_and_queues_tmux() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
        crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

        let app = adw::Application::builder()
            .application_id("com.illya.rttx.local-tmux-bookmark-tests")
            .build();
        app.register(gtk4::gio::Cancellable::NONE).unwrap();

        let window = Window::new(&app);
        let mut bookmark = crate::bookmarks::Bookmark::new("Local Dev");
        bookmark.directory = Some("/home/user/work".into());
        bookmark.tmux_session = Some("dev".into());

        window.new_session_from_bookmark(&bookmark);

        let (layout_cwd, pending_inputs, saved_recovery) = {
            let state = window.imp().state.borrow();
            let session = state.sessions.last().unwrap();
            let terminal_uuid = session.layout.terminal_uuids().into_iter().next().unwrap();
            let layout_cwd = match &session.layout {
                LayoutNode::Terminal { cwd, .. } => cwd.clone(),
                _ => None,
            };
            let term = window.imp().terminals.borrow().get(&terminal_uuid).cloned().unwrap();
            (
                layout_cwd,
                term.pending_shell_inputs_for_test(),
                session.recovery_for(&terminal_uuid).cloned(),
            )
        };

        assert_eq!(layout_cwd.as_deref(), Some("/home/user/work"));
        assert_eq!(pending_inputs, vec!["exec tmux attach-session -t 'dev'\n"]);
        assert_eq!(
            saved_recovery,
            Some(PaneRecovery {
                source: PaneSource::Bookmark { name: "Local Dev".into() },
                target: Some(PaneTarget::LocalTmux { session: "dev".into() }),
                startup: vec![],
            })
        );

        window.close();
        crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
    }

    #[test]
    fn bookmark_active_session_captures_session_name_and_cwd() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
        crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

        let app = adw::Application::builder()
            .application_id("com.illya.rttx.bookmark-active-session-tests")
            .build();
        app.register(gtk4::gio::Cancellable::NONE).unwrap();

        let window = Window::new(&app);

        // Rename the default session and set the terminal's CWD.
        {
            let mut state = window.imp().state.borrow_mut();
            state.sessions[0].name = "My Work".to_string();
        }
        let terminal_uuid = {
            let state = window.imp().state.borrow();
            state.sessions[0].layout.terminal_uuids().into_iter().next().unwrap()
        };
        if let Some(term) = window.imp().terminals.borrow().get(&terminal_uuid) {
            term.set_current_directory_for_test(Some("/home/user/projects"));
        }
        window.imp().focused_terminal_uuid.replace(Some(terminal_uuid));

        let bookmark = window
            .create_bookmark_from_active_session()
            .expect("should produce a bookmark when a terminal is focused");

        assert_eq!(bookmark.name, "My Work", "bookmark name should match the session name");
        assert_eq!(
            bookmark.directory.as_deref(),
            Some("/home/user/projects"),
            "bookmark directory should match the focused terminal's CWD"
        );
        assert_eq!(bookmark.ssh_target, None);
        assert_eq!(bookmark.tmux_session, None);

        window.close();
        crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
    }

    #[test]
    fn bookmark_active_session_returns_none_without_focused_terminal() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
        crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

        let app = adw::Application::builder()
            .application_id("com.illya.rttx.bookmark-no-focus-tests")
            .build();
        app.register(gtk4::gio::Cancellable::NONE).unwrap();

        let window = Window::new(&app);
        window.imp().focused_terminal_uuid.replace(None);

        assert!(
            window.create_bookmark_from_active_session().is_none(),
            "should return None when no terminal is focused"
        );

        window.close();
        crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
    }

    #[test]
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
                        target: Some(PaneTarget::LocalTmux {
                            session: "rttx-definitely-missing-session".into(),
                        }),
                        startup: vec![],
                    },
                )]),
                active_terminal_uuid: Some(terminal_uuid.clone()),
                input_sync: false,
            }],
            ..WindowState::default()
        };
        crate::session::save_window_state(&state).unwrap();

        let app = adw::Application::builder()
            .application_id("com.illya.rttx.recovery-failure-tests")
            .build();
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
        assert!(failed, "missing local tmux session should leave the pane alive and show retry UI");
        assert!(
            term.recovery_message_for_test().contains("Failed to attach local tmux session"),
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
    fn inserted_commands_persist_nonexecuting_recovery_recipe_on_restart() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
        crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

        let app = adw::Application::builder()
            .application_id("com.illya.rttx.command-recovery-tests")
            .build();
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
    fn execute_saved_command_queues_input_before_shell_starts() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
        crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

        let app = adw::Application::builder()
            .application_id("com.illya.rttx.command-queue-tests")
            .build();
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
    fn switching_sessions_focuses_the_visible_terminal() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
        crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

        let app = adw::Application::builder()
            .application_id("com.illya.rttx.session-focus-tests")
            .build();
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
        crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
        crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

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
    fn save_state_updates_nested_terminal_cwds() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
        crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

        let app =
            adw::Application::builder().application_id("com.illya.rttx.save-cwd-tests").build();
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
    fn rename_session_updates_sidebar_and_saved_state() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
        crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

        let app = adw::Application::builder()
            .application_id("com.illya.rttx.rename-session-tests")
            .build();
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

        window.close();
    }

    #[test]
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
            window.lookup_action("add-bookmark").is_some(),
            "add-bookmark action should be registered"
        );
        assert!(
            window.lookup_action("add-command").is_some(),
            "add-command action should be registered"
        );
        assert!(
            window.lookup_action("edit-bookmark").is_some(),
            "edit-bookmark action should be registered"
        );
        assert!(
            window.lookup_action("delete-bookmark").is_some(),
            "delete-bookmark action should be registered"
        );
        assert!(
            window.lookup_action("edit-command").is_some(),
            "edit-command action should be registered"
        );
        assert!(
            window.lookup_action("delete-command").is_some(),
            "delete-command action should be registered"
        );

        window.close();
        crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
    }

    #[test]
    fn bookmark_sidebar_shows_empty_state_when_no_bookmarks() {
        require_display!();

        crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");
        let tmp = tempfile::TempDir::new().unwrap();
        crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());

        let app = adw::Application::builder()
            .application_id("com.illya.rttx.bookmark-empty-state-tests")
            .build();
        app.register(gtk4::gio::Cancellable::NONE).unwrap();
        let window = Window::new(&app);
        window.present();
        pump_events(50);

        assert!(
            window.imp().bookmark_empty.is_visible(),
            "empty state should be visible when no bookmarks"
        );
        assert!(
            !window.imp().bookmark_scroll.is_visible(),
            "list scroll should be hidden when no bookmarks"
        );

        window.close();
        crate::test_helpers::remove_env("RTTX_DISABLE_SHELL_SPAWN");
    }

    #[test]
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
    fn nested_split_preserves_root_and_unaffected_terminals() {
        require_display!();

        let tmp = tempfile::TempDir::new().unwrap();
        crate::test_helpers::set_env("XDG_CONFIG_HOME", tmp.path());
        crate::test_helpers::set_env("RTTX_DISABLE_SHELL_SPAWN", "1");

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

    #[test]
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
                    .max_by_key(|uuid| {
                        state.sessions[0].layout.depth_of_terminal(uuid).unwrap_or(0)
                    })
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
}
