use super::*;

impl Window {
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

    pub(super) fn show_rename_pane_dialog(&self, terminal_uuid: &str, handle: &TerminalHandle) {
        let current_title = handle.title();
        let has_custom = handle.custom_title().is_some();

        let alert = adw::AlertDialog::new(Some("Rename Pane"), None);
        alert.add_response("cancel", "Cancel");
        if has_custom {
            alert.add_response("reset", "Reset");
        }
        alert.add_response("rename", "Rename");
        alert.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
        alert.set_default_response(Some("rename"));
        alert.set_close_response("cancel");

        let entry = adw::EntryRow::builder().title("Pane title").build();
        entry.set_text(&current_title);

        let group = adw::PreferencesGroup::new();
        group.add(&entry);
        alert.set_extra_child(Some(&group));

        alert.connect_response(None, {
            let win = self.clone();
            let uuid = terminal_uuid.to_string();
            move |_, response| match response {
                "rename" => {
                    let text = entry.text().trim().to_string();
                    if !text.is_empty()
                        && let Some(handle) = win.terminal_handle(&uuid)
                    {
                        handle.set_custom_title(Some(&text));
                    }
                }
                "reset" => {
                    if let Some(handle) = win.terminal_handle(&uuid) {
                        handle.set_custom_title(None);
                    }
                }
                _ => {}
            }
        });
        alert.present(Some(self));
    }

    pub(super) fn confirm_close_others(&self, keep_uuid: &str) {
        let other_uuids: Vec<String> = {
            let state = self.imp().state.borrow();
            state
                .workspaces
                .iter()
                .filter(|s| s.uuid != keep_uuid)
                .map(|s| s.uuid.clone())
                .collect()
        };
        if other_uuids.is_empty() {
            return;
        }
        let count = other_uuids.len();
        let body = format!(
            "Close {count} other workspace{}? All panes and running processes in those workspaces will be stopped.",
            if count == 1 { "" } else { "s" }
        );
        let win = self.clone();
        let alert = adw::AlertDialog::new(Some("Close Other Workspaces?"), Some(&body));
        alert.add_response("cancel", "Cancel");
        alert.add_response("close", "Close Others");
        alert.set_response_appearance("close", adw::ResponseAppearance::Destructive);
        alert.set_default_response(Some("cancel"));
        alert.set_close_response("cancel");
        alert.connect_response(None, move |_, response| {
            if response == "close" {
                for uuid in &other_uuids {
                    win.close_session(uuid);
                }
            }
        });
        alert.present(Some(self));
    }

    pub(super) fn confirm_close_all(&self) {
        let count = self.imp().state.borrow().workspaces.len();
        if count == 0 {
            return;
        }
        let body = format!(
            "Close {count} workspace{}? All panes and running processes will be stopped.",
            if count == 1 { "" } else { "s" }
        );
        let win = self.clone();
        let alert = adw::AlertDialog::new(Some("Close All Workspaces?"), Some(&body));
        alert.add_response("cancel", "Cancel");
        alert.add_response("close", "Close All");
        alert.set_response_appearance("close", adw::ResponseAppearance::Destructive);
        alert.set_default_response(Some("cancel"));
        alert.set_close_response("cancel");
        alert.connect_response(None, move |_, response| {
            if response == "close" {
                win.close_all_sessions();
            }
        });
        alert.present(Some(self));
    }

    pub(super) fn confirm_close_session(&self, session_uuid: &str) {
        let should_close_immediately = {
            let state = self.imp().state.borrow();
            state.workspaces.iter().find(|s| s.uuid == session_uuid).is_some_and(|session| {
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

    pub(super) fn confirm_delete_command(&self, uuid: String) {
        let win = self.clone();
        self.confirm_delete(
            "Delete Command?",
            "The command will be permanently removed.",
            move || {
                let store = crate::store::default_store();
                let mut items = store.load_commands();
                items.retain(|c| c.uuid != uuid);
                if let Err(e) = store.save_commands(&items) {
                    tracing::error!("Failed to delete command: {e}");
                }
                win.refresh_command_sidebar();
            },
        );
    }

    pub(super) fn confirm_delete_place(&self, uuid: String) {
        let win = self.clone();
        self.confirm_delete("Delete Place?", "The place will be permanently removed.", move || {
            let store = crate::store::default_store();
            let mut items = store.load_places();
            items.retain(|p| p.uuid != uuid);
            if let Err(e) = store.save_places(&items) {
                tracing::error!("Failed to delete place: {e}");
            }
            win.refresh_place_sidebar();
        });
    }

    pub(super) fn confirm_delete_host(&self, host_key: String) {
        let store = crate::store::default_store();
        let saved_hosts = store.load_hosts().into_value().unwrap_or_default();
        let saved_places = store.load_places();
        let saved_commands = store.load_commands();
        let affected = host::deletion_affected(&host_key, &saved_places, &saved_commands);

        if affected.is_empty() {
            let win = self.clone();
            self.confirm_delete(
                "Delete Host?",
                "The host will be permanently removed.",
                move || {
                    let store = crate::store::default_store();
                    let mut hosts = store.load_hosts().into_value().unwrap_or_default();
                    hosts.retain(|h| h.key != host_key);
                    if let Err(e) = store.save_hosts(&hosts) {
                        tracing::error!("Failed to delete host: {e}");
                    }
                    win.rebuild_host_selector_model(None);
                    win.refresh_place_sidebar();
                    win.refresh_command_sidebar();
                },
            );
            return;
        }

        let resolved = host::resolve(&host_key, &saved_hosts);
        let dialog = adw::Dialog::builder()
            .title(format!("Delete Host: {}?", resolved.name))
            .content_width(480)
            .build();
        let header = adw::HeaderBar::new();
        let delete_button = gtk4::Button::with_label("Delete");
        delete_button.add_css_class("destructive-action");
        header.pack_end(&delete_button);

        let cancel_button = gtk4::Button::with_label("Cancel");
        header.pack_start(&cancel_button);

        let description = gtk4::Label::new(Some(
            "The following places and commands are tagged with this host. \
             Checked items will be deleted. Uncheck items you want to keep \
             (they will appear as orphaned).",
        ));
        description.set_wrap(true);
        description.set_xalign(0.0);
        description.add_css_class("dim-label");

        let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        content_box.set_margin_start(18);
        content_box.set_margin_end(18);
        content_box.set_margin_top(12);
        content_box.set_margin_bottom(18);
        content_box.append(&description);

        let mut place_checks: Vec<(String, gtk4::CheckButton)> = Vec::new();
        let mut command_checks: Vec<(String, gtk4::CheckButton)> = Vec::new();

        if !affected.places.is_empty() {
            let group = adw::PreferencesGroup::new();
            group.set_title("Places");
            for place in &affected.places {
                let check = gtk4::CheckButton::new();
                check.set_active(true);
                let row = adw::ActionRow::builder()
                    .title(&place.name)
                    .subtitle(&place.path)
                    .activatable_widget(&check)
                    .build();
                row.add_prefix(&check);
                group.add(&row);
                place_checks.push((place.uuid.clone(), check));
            }
            content_box.append(&group);
        }

        if !affected.commands.is_empty() {
            let group = adw::PreferencesGroup::new();
            group.set_title("Commands");
            for cmd in &affected.commands {
                let check = gtk4::CheckButton::new();
                check.set_active(true);
                let row = adw::ActionRow::builder()
                    .title(&cmd.title)
                    .subtitle(cmd.preview())
                    .activatable_widget(&check)
                    .build();
                row.add_prefix(&check);
                group.add(&row);
                command_checks.push((cmd.uuid.clone(), check));
            }
            content_box.append(&group);
        }

        let scroll = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vexpand(true)
            .max_content_height(400)
            .propagate_natural_height(true)
            .child(&content_box)
            .build();

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&header);
        toolbar_view.set_content(Some(&scroll));
        dialog.set_child(Some(&toolbar_view));

        let dialog_ref = dialog.clone();
        cancel_button.connect_clicked(move |_| {
            dialog_ref.close();
        });

        let dialog_ref = dialog.clone();
        let win = self.clone();
        delete_button.connect_clicked(move |_| {
            let place_uuids: Vec<String> = place_checks
                .iter()
                .filter(|(_, check)| check.is_active())
                .map(|(uuid, _)| uuid.clone())
                .collect();
            let command_uuids: Vec<String> = command_checks
                .iter()
                .filter(|(_, check)| check.is_active())
                .map(|(uuid, _)| uuid.clone())
                .collect();

            let store = crate::store::default_store();
            let hosts = store.load_hosts().into_value().unwrap_or_default();
            let places_data = store.load_places();
            let commands_data = store.load_commands();
            let (new_hosts, new_places, new_commands) = host::apply_deletion_cleanup(
                &host_key,
                &hosts,
                &places_data,
                &commands_data,
                &place_uuids,
                &command_uuids,
            );
            if let Err(e) = store.save_hosts(&new_hosts) {
                tracing::error!("Failed to save hosts: {e}");
            }
            if let Err(e) = store.save_library(&new_places, &new_commands) {
                tracing::error!("Failed to save library: {e}");
            }
            win.rebuild_host_selector_model(None);
            win.refresh_place_sidebar();
            win.refresh_command_sidebar();
            dialog_ref.close();
        });

        dialog.present(Some(self));
    }

    pub(super) fn show_add_host_dialog(&self) {
        let dialog = adw::Dialog::builder().title("Add Host").content_width(360).build();
        let header = adw::HeaderBar::new();

        let add_button = gtk4::Button::with_label("Add");
        add_button.add_css_class("suggested-action");
        add_button.set_sensitive(false);
        header.pack_end(&add_button);

        let entry = adw::EntryRow::builder().title("SSH target (e.g. user@host)").build();

        let group = adw::PreferencesGroup::new();
        group.add(&entry);

        let content = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        content.set_margin_start(18);
        content.set_margin_end(18);
        content.set_margin_top(18);
        content.set_margin_bottom(18);
        content.append(&group);

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&header);
        toolbar_view.set_content(Some(&content));
        dialog.set_child(Some(&toolbar_view));

        let add_ref = add_button.clone();
        entry.connect_changed(move |e| {
            add_ref.set_sensitive(!e.text().trim().is_empty());
        });

        let dialog_ref = dialog.clone();
        let win = self.clone();
        let entry_ref = entry.clone();
        let commit = move || {
            let text = entry_ref.text();
            let ssh_target = text.trim();
            if ssh_target.is_empty() {
                return;
            }
            let new_host = host::Host::remote(ssh_target);
            let store = crate::store::default_store();
            let mut hosts = store.load_hosts().into_value().unwrap_or_default();
            if hosts.iter().any(|h| h.key == new_host.key) {
                win.show_toast(&format!("Host \"{}\" already exists", new_host.name));
                dialog_ref.close();
                return;
            }
            hosts.push(new_host.clone());
            if let Err(e) = store.save_hosts(&hosts) {
                tracing::error!("Failed to save hosts: {e}");
                win.show_toast("Failed to save host");
                dialog_ref.close();
                return;
            }
            win.rebuild_host_selector_model(Some(&new_host.key));
            win.refresh_place_sidebar();
            win.refresh_command_sidebar();
            win.show_toast(&format!("Host \"{}\" added", new_host.name));
            dialog_ref.close();
        };

        let commit_for_button = commit.clone();
        add_button.connect_clicked(move |_| commit_for_button());
        entry.connect_entry_activated(move |_| commit());

        dialog.present(Some(self));
    }

    pub(super) fn show_about_window(&self) {
        let about = adw::AboutWindow::new();
        about.set_transient_for(Some(self));
        about.set_application_name(config::display_name());
        about.set_application_icon(config::icon_name());
        about.set_version(&format!("{} ({})", env!("CARGO_PKG_VERSION"), env!("GIT_HASH")));
        about.set_developer_name(config::DEVELOPER_NAME);
        about.set_developers(&[config::DEVELOPER_NAME]);
        about.set_website(config::PROJECT_WEBSITE);
        about.set_issue_url(config::ISSUE_TRACKER);
        about.add_link("Support rttx", config::SPONSORS_URL);
        about.set_license_type(gtk4::License::Gpl30);
        about.present();
    }

    pub(super) fn show_workspace_popover_menu(&self, row: &WorkspaceRow, session_uuid: &str) {
        // Unparent any previous popover so it doesn't leak.
        if let Some(old) = self.imp().workspace_popover.borrow_mut().take()
            && old.parent().is_some()
        {
            old.unparent();
        }

        let items = {
            let state = self.imp().state.borrow();
            let Some(session) = state.workspaces.iter().find(|s| s.uuid == session_uuid) else {
                return;
            };
            let statuses = self.imp().workspace_connection_status.borrow();
            let current_status = statuses.get(session_uuid);
            let disconnected = current_status.is_some_and(|s| {
                matches!(
                    s,
                    ConnectionStatus::Disconnected
                        | ConnectionStatus::Reconnecting { .. }
                        | ConnectionStatus::Blocked(_)
                )
            });
            let is_daemon_died = current_status.is_some_and(|s| {
                matches!(s, ConnectionStatus::Blocked(ConnectionProblem::DaemonDied))
            });
            let is_connecting = current_status.is_some_and(|s| {
                matches!(s, ConnectionStatus::Connecting | ConnectionStatus::Starting)
            });
            let endpoint_key = session.runtime.endpoint.key();
            let has_other_disconnected = state.workspaces.iter().any(|s| {
                s.uuid != session_uuid
                    && s.uses_managed_runtime()
                    && s.runtime.endpoint.key() == endpoint_key
                    && statuses.get(&s.uuid).is_some_and(|st| {
                        matches!(
                            st,
                            ConnectionStatus::Disconnected
                                | ConnectionStatus::Reconnecting { .. }
                                | ConnectionStatus::Blocked(_)
                        )
                    })
            });
            drop(statuses);
            crate::runtime::workspace_menu_items(&crate::runtime::WorkspaceMenuContext {
                is_remote: matches!(session.runtime.endpoint, RuntimeEndpoint::Remote { .. }),
                is_managed: session.uses_managed_runtime(),
                is_persistent: session.runtime.is_managed()
                    && matches!(session.runtime.policy, WorkspacePolicy::Persistent),
                is_attached: session.runtime.runtime_id.is_some(),
                is_disconnected: disconnected,
                is_connecting,
                is_daemon_died,
                has_other_disconnected_from_same_host: has_other_disconnected,
            })
        };

        let menu = gtk4::gio::Menu::new();
        menu.append(Some("Rename…"), Some("win.ctx-rename"));
        if items.show_edit_connection {
            menu.append(Some("Edit Connection…"), Some("win.ctx-edit-connection"));
        }
        if items.show_restart_daemon {
            menu.append(Some("Restart Daemon"), Some("win.ctx-restart-daemon"));
        }
        if items.show_reconnect {
            menu.append(Some("Reconnect"), Some("win.ctx-reconnect"));
        }
        if items.show_reconnect_host {
            menu.append(Some("Reconnect All from Host"), Some("win.ctx-reconnect-host"));
        }
        if items.show_detach {
            menu.append(Some("Detach"), Some("win.ctx-detach"));
        }
        menu.append(Some("Close"), Some("win.ctx-close"));
        menu.append(Some("Close Others"), Some("win.ctx-close-others"));
        menu.append(Some("Close All"), Some("win.ctx-close-all"));

        let popover = gtk4::PopoverMenu::from_model(Some(&menu));
        popover.set_has_arrow(true);
        let popover_parent = row.parent().unwrap_or_else(|| row.clone().upcast::<gtk4::Widget>());
        popover.set_parent(&popover_parent);

        let w = self.clone();
        let u = session_uuid.to_string();
        let r = row.clone();
        let rename_action = gtk4::gio::SimpleAction::new("ctx-rename", None);
        rename_action.connect_activate(move |_, _| {
            w.show_rename_runtime_popover(&r, &u);
        });
        self.add_action(&rename_action);

        if items.show_edit_connection {
            let w = self.clone();
            let u = session_uuid.to_string();
            let edit_action = gtk4::gio::SimpleAction::new("ctx-edit-connection", None);
            edit_action.connect_activate(move |_, _| {
                w.show_edit_workspace_connection_dialog(&u);
            });
            self.add_action(&edit_action);
        }

        if items.show_restart_daemon {
            let w = self.clone();
            let u = session_uuid.to_string();
            let restart_action = gtk4::gio::SimpleAction::new("ctx-restart-daemon", None);
            restart_action.connect_activate(move |_, _| {
                w.restart_daemon_and_reconnect(&u);
            });
            self.add_action(&restart_action);
        }

        if items.show_reconnect {
            let w = self.clone();
            let u = session_uuid.to_string();
            let reconnect_action = gtk4::gio::SimpleAction::new("ctx-reconnect", None);
            reconnect_action.connect_activate(move |_, _| {
                w.retry_workspace_connection(&u);
            });
            self.add_action(&reconnect_action);
        }

        if items.show_reconnect_host {
            let w = self.clone();
            let u = session_uuid.to_string();
            let reconnect_host_action = gtk4::gio::SimpleAction::new("ctx-reconnect-host", None);
            reconnect_host_action.connect_activate(move |_, _| {
                w.retry_all_workspaces_for_endpoint(&u);
            });
            self.add_action(&reconnect_host_action);
        }

        if items.show_detach {
            let w = self.clone();
            let u = session_uuid.to_string();
            let detach_action = gtk4::gio::SimpleAction::new("ctx-detach", None);
            detach_action.connect_activate(move |_, _| {
                w.detach_runtime(&u);
            });
            self.add_action(&detach_action);
        }

        let w = self.clone();
        let u = session_uuid.to_string();
        let close_action = gtk4::gio::SimpleAction::new("ctx-close", None);
        close_action.connect_activate(move |_, _| {
            w.confirm_close_session(&u);
        });
        self.add_action(&close_action);

        let w = self.clone();
        let u = session_uuid.to_string();
        let close_others_action = gtk4::gio::SimpleAction::new("ctx-close-others", None);
        close_others_action.connect_activate(move |_, _| {
            w.confirm_close_others(&u);
        });
        self.add_action(&close_others_action);

        let w = self.clone();
        let close_all_action = gtk4::gio::SimpleAction::new("ctx-close-all", None);
        close_all_action.connect_activate(move |_, _| {
            w.confirm_close_all();
        });
        self.add_action(&close_all_action);

        self.imp().workspace_popover.replace(Some(popover.clone()));
        popover.popup();
    }

    pub(super) fn show_rename_runtime_popover(&self, row: &WorkspaceRow, session_uuid: &str) {
        let current_name = {
            let state = self.imp().state.borrow();
            let Some(session) =
                state.workspaces.iter().find(|session| session.uuid == session_uuid)
            else {
                return;
            };
            session.name.clone()
        };

        let popover = gtk4::Popover::new();
        popover.set_has_arrow(true);
        popover.set_position(gtk4::PositionType::Bottom);
        // Parent the popover on the wrapper ListBoxRow (row's parent), not the
        // WorkspaceRow itself, because WorkspaceRow is a ListBoxRow subclass that
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
                win.rename_runtime(&session_uuid, &name);
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

    pub(crate) fn rename_runtime(&self, session_uuid: &str, new_name: &str) {
        let runtime_info = {
            let mut state = self.imp().state.borrow_mut();
            let Some(session) =
                state.workspaces.iter_mut().find(|session| session.uuid == session_uuid)
            else {
                return;
            };
            session.name = new_name.to_string();
            session.user_renamed = true;
            session
                .runtime
                .managed
                .then(|| (session.runtime.endpoint.clone(), session.runtime.runtime_id.clone()))
        };

        if let Some((endpoint, Some(runtime_id))) = runtime_info
            && let Some(manager) = self.imp().connection_manager.borrow().as_ref()
        {
            manager.rename_runtime(session_uuid, &endpoint, &runtime_id, new_name);
        }

        if let Some(session_row) = self.sidebar_workspace_row(session_uuid) {
            session_row.set_workspace_name(new_name);
        }

        // Persist immediately rather than waiting for the auto-save timer or a
        // clean window close: a rename is a deliberate, cheap-to-save user
        // action, and losing it to a crash or a session-manager kill is the
        // "name reverts after restart" symptom of issue #1084.
        self.save_state();
    }

    /// Extract the SSH target from the active session's remote endpoint.
    ///
    /// Returns `None` for local or unmanaged sessions.
    pub(crate) fn ssh_target_for_active_session(&self) -> Option<String> {
        let state = self.imp().state.borrow();
        let visible = self.imp().session_stack.visible_child_name()?;
        let session = state.workspaces.iter().find(|s| s.uuid == visible.as_str())?;
        match &session.runtime.endpoint {
            RuntimeEndpoint::Remote { host, .. } if session.runtime.is_managed() => {
                Some(host.clone())
            }
            _ => None,
        }
    }

    pub(super) fn do_add_current_host(&self) {
        let Some(ssh_target) = self.ssh_target_for_active_session() else {
            self.show_toast("No SSH host in the active workspace");
            return;
        };

        let new_host = host::Host::remote(&ssh_target);
        let store = crate::store::default_store();
        let mut hosts = store.load_hosts().into_value().unwrap_or_default();

        if hosts.iter().any(|h| h.key == new_host.key) {
            self.show_toast(&format!("Host \"{}\" is already saved", new_host.name));
            return;
        }

        hosts.push(new_host.clone());
        if let Err(e) = store.save_hosts(&hosts) {
            tracing::error!("Failed to save hosts: {e}");
            self.show_toast("Failed to save host");
            return;
        }

        self.rebuild_host_selector_model(None);
        self.show_toast(&format!("Host \"{}\" added", new_host.name));
    }

    pub(super) fn do_add_current_path_to_places(&self) {
        let Some(uuid) = self.focused_terminal_uuid() else { return };
        let cwd = self.terminal_handle(&uuid).and_then(|t| t.current_directory());
        let Some(cwd) = cwd else {
            self.show_toast("No working directory available");
            return;
        };

        let host_key = {
            let state = self.imp().state.borrow();
            let visible = self.imp().session_stack.visible_child_name();
            visible
                .and_then(|name| state.workspaces.iter().find(|s| s.uuid == name.as_str()))
                .map_or_else(|| host::LOCAL_KEY.into(), |s| s.runtime.endpoint.host_key())
        };

        let host_tags = if host_key == host::LOCAL_KEY { vec![] } else { vec![host_key] };

        let place = crate::places::Place::from_cwd(&cwd, host_tags);
        let label = place.display_label();
        let store = crate::store::default_store();
        let mut places = store.load_places();
        places.push(place);
        if let Err(e) = store.save_places(&places) {
            tracing::error!("Failed to save places: {e}");
            self.show_toast("Failed to save place");
            return;
        }

        self.refresh_place_sidebar();
        self.show_toast(&format!("Place \"{label}\" added"));
    }

    pub(super) fn confirm_paste(&self, terminal_uuid: &str, text: &str) {
        use crate::terminal::paste_guard::{analyse, flatten_to_single_line};

        let analysis = analyse(text);
        let alert = build_paste_guard_dialog(&analysis);

        let win = self.clone();
        let uuid = terminal_uuid.to_string();
        let original_text = text.to_string();
        alert.connect_response(None, move |_, response| {
            let Some(terminal) = win.terminal_handle(&uuid) else { return };
            match response {
                "paste" => {
                    Self::execute_paste_text(&terminal, &win, &uuid, &original_text);
                }
                "single-line" => {
                    let flat = flatten_to_single_line(&original_text);
                    Self::execute_paste_text(&terminal, &win, &uuid, &flat);
                }
                _ => {}
            }
        });
        alert.present(Some(self));
    }
}

/// Build the paste guard confirmation dialog.
///
/// Extracted so the dialog configuration can be tested without a full Window.
pub(crate) fn build_paste_guard_dialog(
    analysis: &crate::terminal::paste_guard::PasteAnalysis,
) -> adw::AlertDialog {
    let body = format!(
        "{} lines, {} bytes\n\n{}",
        analysis.line_count, analysis.byte_len, analysis.preview
    );
    let alert = adw::AlertDialog::new(Some("Confirm Paste"), Some(&body));
    alert.add_response("cancel", "Cancel");
    alert.add_response("single-line", "Paste as Single Line");
    alert.add_response("paste", "Paste");
    alert.set_response_appearance("paste", adw::ResponseAppearance::Suggested);
    alert.set_default_response(Some("paste"));
    alert.set_close_response("cancel");
    alert
}
