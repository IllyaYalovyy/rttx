use super::*;

impl Window {
    pub(super) fn setup_actions(&self, app: &adw::Application) {
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

    fn toggle_focused_search(&self) {
        if let Some(uuid) = self.focused_terminal_uuid()
            && let Some(terminal) = self.terminal_handle(&uuid)
        {
            terminal.toggle_search();
        }
    }

    pub(super) fn swap_terminals(&self, uuid_a: &str, uuid_b: &str) {
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

    pub(super) fn toggle_pane_zoom(&self) {
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

    pub(super) fn focused_terminal_uuid(&self) -> Option<String> {
        self.imp().focused_terminal_uuid.borrow().clone()
    }

    pub(super) fn terminal_handle(&self, terminal_uuid: &str) -> Option<TerminalHandle> {
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

    pub(super) fn set_focused_terminal(&self, terminal_uuid: Option<&str>) {
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

    pub(crate) fn show_toast(&self, message: &str) {
        self.imp().toast_overlay.add_toast(adw::Toast::new(message));
    }

    pub(super) fn cycle_session(&self, delta: i32) {
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

    pub(super) fn switch_to_session_number(&self, number: usize) {
        let Some(index) = number.checked_sub(1) else {
            return;
        };

        if let Some(row) = self.imp().sidebar_list.row_at_index(index as i32) {
            self.imp().sidebar_list.select_row(Some(&row));
        }
    }
}
