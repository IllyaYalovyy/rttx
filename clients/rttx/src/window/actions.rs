use super::*;
use crate::leader::{self, LeaderMatch};
use crate::shortcuts;
use crate::terminal::paste_guard::{PasteGuardDecision, decide};

type ActionCallback = fn(&Window);

impl Window {
    pub(super) fn setup_actions(&self, app: &adw::Application) {
        let prefs =
            crate::store::default_store().load_preferences().into_value().unwrap_or_default();
        let overrides = &prefs.keyboard_shortcuts;

        let actions: &[(&str, ActionCallback)] = &[
            ("close-terminal", Self::close_focused_terminal),
            ("split-horizontal", |w| {
                w.split_focused(SplitOrientation::Horizontal);
            }),
            ("split-vertical", |w| {
                w.split_focused(SplitOrientation::Vertical);
            }),
            ("search", Self::toggle_focused_search),
            ("copy", Self::clipboard_copy),
            ("paste", Self::clipboard_paste),
            ("prev-session", |w| w.cycle_session(-1)),
            ("next-session", |w| w.cycle_session(1)),
            ("toggle-sidebar", |w| {
                let panel = w.imp().left_paned.start_child().expect("left sidebar panel");
                panel.set_visible(!panel.is_visible());
            }),
            ("fullscreen", |w| {
                if w.is_fullscreen() {
                    w.unfullscreen();
                } else {
                    w.fullscreen();
                }
            }),
            ("zoom-in", |w| w.zoom_focused(1)),
            ("zoom-out", |w| w.zoom_focused(-1)),
            ("zoom-reset", |w| w.zoom_focused(0)),
            ("toggle-pane-zoom", Self::toggle_pane_zoom),
            ("rotate-layout", Self::rotate_layout),
            ("repair-terminal", Self::repair_focused_terminal),
            ("rename-pane", Self::rename_focused_pane),
            ("rename-workspace", Self::rename_active_workspace),
            ("new-session", Self::add_session),
            ("new-ephemeral-workspace", Self::add_ephemeral_session),
            ("new-remote-workspace", Self::show_new_remote_workspace_dialog),
            ("browse-remote-runtimes", Self::show_browse_remote_runtimes_dialog),
            ("connect-existing-local", Self::connect_existing_local),
            ("connect-to-existing", Self::connect_existing_local),
            ("new-direct", |w| {
                w.add_direct_session();
            }),
            ("toggle-utility-sidebar", |w| {
                let sidebar = &w.imp().utility_sidebar_box;
                sidebar.set_visible(!sidebar.is_visible());
            }),
            ("add-current-host", Self::do_add_current_host),
            ("add-current-place", Self::do_add_current_path_to_places),
            ("add-command", |w| {
                crate::commands_window::show_form(w, None);
            }),
            ("add-place", |w| {
                crate::places_window::show_form(w, None);
            }),
        ];

        for (name, callback) in actions {
            let action = gtk4::gio::SimpleAction::new(name, None);
            let win = self.clone();
            let cb = *callback;
            action.connect_activate(move |_, _| {
                cb(&win);
            });
            self.add_action(&action);
            let accels = shortcuts::effective_accels(name, overrides);
            let accel_refs: Vec<&str> = accels.iter().map(AsRef::as_ref).collect();
            app.set_accels_for_action(&format!("win.{name}"), &accel_refs);
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
        {
            let accels = shortcuts::effective_accels("toggle-input-sync", overrides);
            let accel_refs: Vec<&str> = accels.iter().map(AsRef::as_ref).collect();
            app.set_accels_for_action("win.toggle-input-sync", &accel_refs);
        }

        let no_persist_action = gtk4::gio::SimpleAction::new("toggle-no-persist", None);
        let win = self.clone();
        no_persist_action.connect_activate(move |_, _| {
            win.toggle_focused_pane_no_persist();
        });
        self.add_action(&no_persist_action);

        let prefs_action = gtk4::gio::SimpleAction::new("preferences", None);
        let win = self.clone();
        prefs_action.connect_activate(move |_, _| {
            crate::preferences_window::show(&win);
        });
        self.add_action(&prefs_action);
        {
            let accels = shortcuts::effective_accels("preferences", overrides);
            let accel_refs: Vec<&str> = accels.iter().map(AsRef::as_ref).collect();
            app.set_accels_for_action("win.preferences", &accel_refs);
        }

        let edit_command_action =
            gtk4::gio::SimpleAction::new("edit-command", Some(glib::VariantTy::STRING));
        let win = self.clone();
        edit_command_action.connect_activate(move |_, param| {
            let uuid: String = param.and_then(glib::Variant::get).unwrap_or_default();
            let all_commands = crate::store::default_store().load_commands();
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

        let duplicate_command_action =
            gtk4::gio::SimpleAction::new("duplicate-command", Some(glib::VariantTy::STRING));
        let win = self.clone();
        duplicate_command_action.connect_activate(move |_, param| {
            let uuid: String = param.and_then(glib::Variant::get).unwrap_or_default();
            let all_commands = crate::store::default_store().load_commands();
            if let Some(command) = all_commands.iter().find(|c| c.uuid == uuid) {
                let copy = command.duplicate();
                let mut items = crate::store::default_store().load_commands();
                items.push(copy.clone());
                let _ = crate::store::default_store().save_commands(&items);
                win.refresh_command_sidebar();
                crate::commands_window::show_form(&win, Some(&copy));
            }
        });
        self.add_action(&duplicate_command_action);

        let copy_body_action =
            gtk4::gio::SimpleAction::new("copy-command-body", Some(glib::VariantTy::STRING));
        let win = self.clone();
        copy_body_action.connect_activate(move |_, param| {
            let uuid: String = param.and_then(glib::Variant::get).unwrap_or_default();
            let all_commands = crate::store::default_store().load_commands();
            if let Some(command) = all_commands.iter().find(|c| c.uuid == uuid)
                && let Some(display) = gtk4::gdk::Display::default()
            {
                display.clipboard().set_text(&command.body);
                win.show_toast("Copied to clipboard");
            }
        });
        self.add_action(&copy_body_action);

        let edit_place_action =
            gtk4::gio::SimpleAction::new("edit-place", Some(glib::VariantTy::STRING));
        let win = self.clone();
        edit_place_action.connect_activate(move |_, param| {
            let uuid: String = param.and_then(glib::Variant::get).unwrap_or_default();
            let all_places = crate::store::default_store().load_places();
            if let Some(place) = all_places.iter().find(|p| p.uuid == uuid) {
                crate::places_window::show_form(&win, Some(place));
            }
        });
        self.add_action(&edit_place_action);

        let delete_place_action =
            gtk4::gio::SimpleAction::new("delete-place", Some(glib::VariantTy::STRING));
        let win = self.clone();
        delete_place_action.connect_activate(move |_, param| {
            let uuid: String = param.and_then(glib::Variant::get).unwrap_or_default();
            if !uuid.is_empty() {
                win.confirm_delete_place(uuid);
            }
        });
        self.add_action(&delete_place_action);

        let open_place_action =
            gtk4::gio::SimpleAction::new("open-place", Some(glib::VariantTy::STRING));
        let win = self.clone();
        open_place_action.connect_activate(move |_, param| {
            let path: String = param.and_then(glib::Variant::get).unwrap_or_default();
            if !path.is_empty() {
                win.open_place_in_current_pane(&path);
            }
        });
        self.add_action(&open_place_action);

        let about_action = gtk4::gio::SimpleAction::new("about", None);
        let win = self.clone();
        about_action.connect_activate(move |_, _| {
            win.show_about_window();
        });
        self.add_action(&about_action);

        // Pane navigation — shortcuts are customizable via preferences.
        {
            let nav_actions: &[(&str, Direction)] = &[
                ("navigate-left", Direction::Left),
                ("navigate-right", Direction::Right),
                ("navigate-up", Direction::Up),
                ("navigate-down", Direction::Down),
            ];
            for (name, direction) in nav_actions {
                let action = gtk4::gio::SimpleAction::new(name, None);
                let win = self.clone();
                let dir = *direction;
                action.connect_activate(move |_, _| win.navigate_focused(dir));
                self.add_action(&action);
                let accels = shortcuts::effective_accels(name, overrides);
                let accel_refs: Vec<&str> = accels.iter().map(AsRef::as_ref).collect();
                app.set_accels_for_action(&format!("win.{name}"), &accel_refs);
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
                .workspaces
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

    fn repair_focused_terminal(&self) {
        if let Some(uuid) = self.focused_terminal_uuid()
            && let Some(terminal) = self.terminal_handle(&uuid)
        {
            terminal.repair_terminal();
            self.show_toast("Terminal repaired");
        }
    }

    fn rename_focused_pane(&self) {
        if let Some(uuid) = self.focused_terminal_uuid()
            && let Some(terminal) = self.terminal_handle(&uuid)
        {
            self.show_rename_pane_dialog(&uuid, &terminal);
        }
    }

    fn rename_active_workspace(&self) {
        let imp = self.imp();
        if let Some(row) = imp
            .sidebar_list
            .selected_row()
            .and_then(|r| r.child())
            .and_then(|c| c.downcast::<WorkspaceRow>().ok())
        {
            let uuid = row.uuid();
            self.show_rename_runtime_popover(&row, &uuid);
        }
    }

    /// Rename the focused pane directly (for D-Bus / test automation).
    /// Empty string clears the custom title.
    pub(crate) fn rename_focused_pane_direct(&self, name: &str) {
        if let Some(uuid) = self.focused_terminal_uuid()
            && let Some(handle) = self.terminal_handle(&uuid)
        {
            if name.is_empty() {
                handle.set_custom_title(None);
            } else {
                handle.set_custom_title(Some(name));
            }
        }
    }

    pub(super) fn toggle_focused_pane_no_persist(&self) {
        let Some(terminal_uuid) = self.focused_terminal_uuid() else { return };
        let Some((workspace_id, endpoint, runtime_id, runtime_pane_id)) =
            self.managed_binding_for_terminal(&terminal_uuid)
        else {
            return;
        };
        if let Some(manager) = self.imp().connection_manager.borrow().as_ref() {
            // Toggle: we don't track the current state client-side, so we
            // always send true. The server is the source of truth and the
            // PaneInfo in the next inventory refresh will reflect the new
            // state. For a proper toggle we'd need to read the current
            // value from the server, but fire-and-forget is simpler and
            // matches the SetPaneTitle pattern.
            manager.set_pane_no_persist(
                &workspace_id,
                &endpoint,
                &runtime_id,
                &runtime_pane_id,
                true,
            );
            let toast =
                libadwaita::Toast::new("Confidential mode enabled — scrollback will not be saved");
            toast.set_timeout(3);
            self.imp().toast_overlay.add_toast(toast);
        }
    }

    pub(super) fn swap_terminals(&self, uuid_a: &str, uuid_b: &str) {
        let imp = self.imp();
        let (session_uuid, session_state) = {
            let mut state = imp.state.borrow_mut();
            let session = state
                .workspaces
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
        let target = self.focused_terminal_uuid();
        self.toggle_pane_zoom_for(target.as_deref());
    }

    pub(super) fn toggle_pane_zoom_for(&self, target_uuid: Option<&str>) {
        let Some(session_uuid) =
            self.imp().session_stack.visible_child_name().map(|n| n.to_string())
        else {
            return;
        };
        let session_state = {
            let mut state = self.imp().state.borrow_mut();
            let Some(session) = state.workspaces.iter_mut().find(|s| s.uuid == session_uuid) else {
                return;
            };
            if session.is_zoomed() {
                session.zoomed_terminal_uuid = None;
            } else {
                let Some(target) = target_uuid else {
                    return;
                };
                if session.layout.terminal_count() < 2 {
                    return;
                }
                session.zoomed_terminal_uuid = Some(target.to_owned());
            }
            session.clone()
        };
        self.rebuild_session_content(&session_uuid, &session_state);
        self.focus_session_terminal(&session_uuid);
    }

    fn rotate_layout(&self) {
        let Some(session_uuid) =
            self.imp().session_stack.visible_child_name().map(|n| n.to_string())
        else {
            return;
        };
        let session_state = {
            let mut state = self.imp().state.borrow_mut();
            let Some(session) = state.workspaces.iter_mut().find(|s| s.uuid == session_uuid) else {
                return;
            };
            session.layout = session.layout.rotated();
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
        let Some(uuid) = self.focused_terminal_uuid() else { return };
        let Some(terminal) = self.terminal_handle(&uuid) else { return };

        let prefs =
            crate::store::default_store().load_preferences().into_value().unwrap_or_default();
        if !prefs.paste_guard {
            Self::execute_paste(&terminal, self, &uuid);
            return;
        }

        let Some(display) = gtk4::gdk::Display::default() else {
            Self::execute_paste(&terminal, self, &uuid);
            return;
        };
        let clipboard = display.clipboard();
        let is_direct = matches!(terminal, TerminalHandle::Direct(_));
        let win = self.clone();
        let terminal_uuid = uuid;
        clipboard.read_text_async(None::<&gtk4::gio::Cancellable>, move |result| {
            let clipboard_text = match result {
                Ok(Some(ref t)) if !t.is_empty() => Some(t.as_str()),
                _ => None,
            };

            let threshold = crate::store::default_store()
                .load_preferences()
                .into_value()
                .unwrap_or_default()
                .paste_guard_threshold;
            match decide(clipboard_text, threshold, is_direct) {
                PasteGuardDecision::Paste | PasteGuardDecision::FallThroughToVte => {
                    if let Some(terminal) = win.terminal_handle(&terminal_uuid) {
                        Self::execute_paste(&terminal, &win, &terminal_uuid);
                    }
                }
                PasteGuardDecision::Confirm => {
                    if let Some(text) = clipboard_text {
                        win.confirm_paste(&terminal_uuid, text);
                    }
                }
                PasteGuardDecision::Skip => {}
            }
        });
    }

    fn execute_paste(terminal: &TerminalHandle, win: &Self, terminal_uuid: &str) {
        match terminal {
            TerminalHandle::Direct(terminal) => terminal.vte().paste_clipboard(),
            TerminalHandle::Managed(pane) => {
                let win = win.clone();
                let terminal_uuid = terminal_uuid.to_string();
                pane.request_clipboard_paste(move |bytes| {
                    win.send_managed_terminal_input(&terminal_uuid, &bytes);
                });
            }
        }
    }

    pub(super) fn execute_paste_text(
        terminal: &TerminalHandle,
        win: &Self,
        terminal_uuid: &str,
        text: &str,
    ) {
        match terminal {
            TerminalHandle::Direct(t) => {
                let bytes = crate::terminal::persistent_widget::pastify(text.as_bytes());
                t.vte().feed_child(&bytes);
            }
            TerminalHandle::Managed(pane) => {
                let bytes =
                    crate::terminal::persistent_widget::pastify_for_pane(pane, text.as_bytes());
                let win = win.clone();
                let terminal_uuid = terminal_uuid.to_string();
                win.send_managed_terminal_input(&terminal_uuid, &bytes);
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
        let count = state.workspaces.len() as i32;
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

    // ── Leader-prefix command shortcuts ─────────────────────

    pub(super) fn setup_leader_controller(&self) {
        let prefs =
            crate::store::default_store().load_preferences().into_value().unwrap_or_default();
        let leader_accels =
            shortcuts::effective_accels("commands-leader", &prefs.keyboard_shortcuts);
        if leader_accels.is_empty() {
            return;
        }

        let leader_action = gtk4::gio::SimpleAction::new("commands-leader", None);
        let win = self.clone();
        leader_action.connect_activate(move |_, _| {
            win.activate_leader_mode();
        });
        self.add_action(&leader_action);

        if let Some(app) = self.application().and_downcast::<adw::Application>() {
            let accel_refs: Vec<&str> = leader_accels.iter().map(AsRef::as_ref).collect();
            app.set_accels_for_action("win.commands-leader", &accel_refs);
        }
    }

    fn activate_leader_mode(&self) {
        let imp = self.imp();
        imp.leader_keys.borrow_mut().clear();
        if let Some(source) = imp.leader_timeout_source.take() {
            source.remove();
        }

        self.show_toast("Leader active — press a key");

        let win = self.clone();
        let source = glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || {
            win.cancel_leader_mode();
        });
        imp.leader_timeout_source.replace(Some(source));

        let controller = gtk4::EventControllerKey::new();
        let win = self.clone();
        controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
        controller.connect_key_pressed(move |ctrl, keyval, _keycode, _state| {
            let key_name = keyval.name().map(|n| n.to_string()).unwrap_or_default();
            if key_name.is_empty() || key_name == "Escape" {
                win.cancel_leader_mode();
                win.remove_controller(ctrl);
                return glib::Propagation::Stop;
            }

            win.imp().leader_keys.borrow_mut().push(key_name);
            let keys = win.imp().leader_keys.borrow().clone();
            let host_key = win.selected_host_key();
            let commands = crate::store::default_store().load_commands();

            match leader::resolve(&commands, &keys, host_key.as_deref()) {
                LeaderMatch::Complete(uuid) => {
                    win.finish_leader_mode();
                    win.remove_controller(ctrl);
                    if let Some(cmd) = commands.iter().find(|c| c.uuid == uuid) {
                        win.execute_saved_command(cmd, cmd.default_run_mode);
                    }
                }
                LeaderMatch::Partial => {
                    // Wait for more keys
                }
                LeaderMatch::NoMatch => {
                    win.cancel_leader_mode();
                    win.remove_controller(ctrl);
                }
            }
            glib::Propagation::Stop
        });
        self.add_controller(controller);
    }

    fn cancel_leader_mode(&self) {
        self.finish_leader_mode();
    }

    fn finish_leader_mode(&self) {
        let imp = self.imp();
        imp.leader_keys.borrow_mut().clear();
        if let Some(source) = imp.leader_timeout_source.take() {
            source.remove();
        }
    }

    /// Whether leader mode is currently active.
    #[must_use]
    pub fn is_leader_active(&self) -> bool {
        self.imp().leader_timeout_source.borrow().is_some()
    }
}
