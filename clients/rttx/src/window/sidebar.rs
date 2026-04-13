use super::*;

/// Sentinel value for the "All Hosts" entry in the host selector.
const ALL_HOSTS_LABEL: &str = "All Hosts";

impl Window {
    /// Return the host key currently selected in the host dropdown,
    /// or `None` when "All Hosts" is selected.
    pub(super) fn selected_host_key(&self) -> Option<String> {
        let dd = &self.imp().host_selector;
        let idx = dd.selected() as usize;
        let keys = self.imp().host_selector_keys.borrow();
        keys.get(idx).cloned()
    }

    /// Rebuild the host selector model and select the entry matching `host_key`.
    pub(crate) fn sync_host_selector_to_workspace(&self, session_uuid: &str) {
        let host_key = {
            let state = self.imp().state.borrow();
            state
                .sessions
                .iter()
                .find(|s| s.uuid == session_uuid)
                .map_or_else(|| host::LOCAL_KEY.into(), |s| s.runtime.endpoint.host_key())
        };
        self.rebuild_host_selector_model(Some(&host_key));
    }

    /// Show the delete button only when a deletable remote host is selected.
    pub(crate) fn update_host_delete_button_visibility(&self) {
        let visible = self.selected_host_key().is_some_and(|key| key != host::LOCAL_KEY);
        self.imp().host_delete_button.set_visible(visible);
    }

    /// Rebuild the host selector dropdown model from saved hosts + workspace endpoints.
    pub(super) fn rebuild_host_selector_model(&self, select_key: Option<&str>) {
        let saved_hosts = host::load();
        let state = self.imp().state.borrow();

        let mut keys: Vec<String> = Vec::new();
        keys.push(host::LOCAL_KEY.into());
        for h in &saved_hosts {
            if !keys.contains(&h.key) {
                keys.push(h.key.clone());
            }
        }
        for s in &state.sessions {
            let k = s.runtime.endpoint.host_key();
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        drop(state);

        let mut labels: Vec<String> = keys
            .iter()
            .map(|k| {
                let resolved = host::resolve(k, &saved_hosts);
                resolved.name
            })
            .collect();
        labels.push(ALL_HOSTS_LABEL.into());

        // Store keys for index-based lookup (None for "All Hosts")
        self.imp().host_selector_keys.replace(keys.clone());

        let model = gtk4::StringList::new(&labels.iter().map(String::as_str).collect::<Vec<_>>());
        let dd = &self.imp().host_selector;
        dd.set_model(Some(&model));

        let target_idx = select_key.and_then(|key| keys.iter().position(|k| k == key)).unwrap_or(0);
        dd.set_selected(target_idx as u32);

        self.refresh_host_menus();
    }

    pub(crate) fn refresh_place_sidebar(&self) {
        let imp = self.imp();
        while let Some(row) = imp.place_list.row_at_index(0) {
            imp.place_list.remove(&row);
        }

        let query = imp.sidebar_search_entry.text();
        let saved = places::load();
        let saved_hosts = host::load();
        let selected_key = self.selected_host_key();

        let any_shown = selected_key.as_ref().map_or_else(
            || {
                // All Hosts view: group by host key
                let mut shown = false;
                let all_keys = self.collect_all_host_keys(&saved_hosts);
                for key in &all_keys {
                    let resolved = host::resolve(key, &saved_hosts);
                    let visible: Vec<_> =
                        saved.iter().filter(|p| p.host_tags.iter().any(|t| t == key)).collect();
                    if !visible.is_empty() {
                        let is_orphaned = !self.is_known_host_key(key, &saved_hosts);
                        let label = if is_orphaned {
                            format!("{} (orphaned)", resolved.name)
                        } else {
                            resolved.name.clone()
                        };
                        shown |= self.append_place_section(&label, &visible, query.as_str());
                    }
                }
                let global_items: Vec<places::Place> = places::builtins()
                    .into_iter()
                    .chain(saved.iter().filter(|p| p.host_tags.is_empty()).cloned())
                    .collect();
                let global_refs: Vec<&places::Place> = global_items.iter().collect();
                shown |= self.append_place_section("Global", &global_refs, query.as_str());
                shown
            },
            |key| {
                let visible = places::visible_for_host(&saved, key);
                let (host_specific, global): (Vec<_>, Vec<_>) =
                    visible.iter().partition(|p| !p.host_tags.is_empty());

                let mut shown = false;
                if !host_specific.is_empty() {
                    let resolved = host::resolve(key, &saved_hosts);
                    shown |=
                        self.append_place_section(&resolved.name, &host_specific, query.as_str());
                }
                shown |= self.append_place_section("Global", &global, query.as_str());
                shown
            },
        );

        imp.place_scroll.set_visible(any_shown);
        imp.place_empty.set_visible(!any_shown);
    }

    fn append_place_section(
        &self,
        section_label: &str,
        items: &[&places::Place],
        query: &str,
    ) -> bool {
        let imp = self.imp();
        let filtered: Vec<_> = items.iter().filter(|p| places::matches_query(p, query)).collect();
        if filtered.is_empty() {
            return false;
        }

        let label = gtk4::Label::new(Some(section_label));
        label.set_xalign(0.0);
        label.add_css_class("dim-label");
        label.add_css_class("caption");
        label.set_margin_start(6);
        label.set_margin_top(8);
        label.set_margin_bottom(2);
        let label_row = gtk4::ListBoxRow::new();
        label_row.set_child(Some(&label));
        label_row.set_selectable(false);
        label_row.set_activatable(false);
        imp.place_list.append(&label_row);

        for place in &filtered {
            let action_row = adw::ActionRow::new();
            action_row.set_title(&place.name);
            if place.name != place.path {
                action_row.set_subtitle(&place.path);
            }
            action_row.set_activatable(true);

            let is_builtin = place.uuid.starts_with("builtin:");
            if !is_builtin {
                let uuid = place.uuid.clone();
                let edit_item = gtk4::gio::MenuItem::new(Some("Edit"), None);
                edit_item.set_action_and_target_value(
                    Some("win.edit-place"),
                    Some(&uuid.to_variant()),
                );
                let delete_item = gtk4::gio::MenuItem::new(Some("Delete"), None);
                delete_item.set_action_and_target_value(
                    Some("win.delete-place"),
                    Some(&uuid.to_variant()),
                );
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
                action_row.add_suffix(&more_button);
            }

            let win = self.clone();
            let path = place.path.clone();
            action_row.connect_activated(move |_| {
                win.open_place_in_current_pane(&path);
            });

            imp.place_list.append(&action_row);
        }
        true
    }

    pub(super) fn open_place_in_current_pane(&self, path: &str) {
        let Some(terminal_uuid) = self.command_target_terminal_uuid() else {
            return;
        };
        let resolved = crate::new_workspace_dialog::resolve_place_path_public(path);
        let cd_path = resolved.as_deref().unwrap_or("~");
        self.send_input_to_terminal(&terminal_uuid, &format!("cd {cd_path}\n"));
    }

    pub(crate) fn refresh_command_sidebar(&self) {
        let imp = self.imp();
        while let Some(row) = imp.command_list.row_at_index(0) {
            imp.command_list.remove(&row);
        }

        let query = imp.command_search_entry.text();
        let sidebar_query = imp.sidebar_search_entry.text();
        let combined_query = if sidebar_query.is_empty() {
            query.to_string()
        } else if query.is_empty() {
            sidebar_query.to_string()
        } else {
            format!("{sidebar_query} {query}")
        };

        let all_commands = commands::load();
        let selected_key = self.selected_host_key();

        let filtered: Vec<_> = match &selected_key {
            Some(key) => commands::visible_for_host(&all_commands, key),
            None => all_commands,
        }
        .into_iter()
        .filter(|command| commands::matches_query(command, &combined_query))
        .collect();

        for command in &filtered {
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
            let command_clone = command.clone();
            insert_button.connect_clicked(move |_| {
                win.execute_saved_command(&command_clone, CommandRunMode::Insert);
            });
        }

        let is_empty = imp.command_list.row_at_index(0).is_none();
        imp.command_scroll.set_visible(!is_empty);
        imp.command_empty.set_visible(is_empty);
    }

    /// Collect all host keys referenced by places and commands.
    fn collect_all_host_keys(&self, saved_hosts: &[host::Host]) -> Vec<String> {
        let mut keys: Vec<String> = Vec::new();
        for h in saved_hosts {
            if !keys.contains(&h.key) {
                keys.push(h.key.clone());
            }
        }
        for p in places::load() {
            for tag in &p.host_tags {
                if !keys.contains(tag) {
                    keys.push(tag.clone());
                }
            }
        }
        for c in commands::load() {
            for tag in &c.host_tags {
                if !keys.contains(tag) {
                    keys.push(tag.clone());
                }
            }
        }
        keys
    }

    /// Whether a host key is known (saved or is the local key).
    fn is_known_host_key(&self, key: &str, saved_hosts: &[host::Host]) -> bool {
        key == host::LOCAL_KEY || saved_hosts.iter().any(|h| h.key == key)
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

    pub(super) fn mark_session_activity(&self, terminal_uuid: &str) {
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

    pub(super) fn notify_process_completed(&self, terminal_uuid: &str, status: i32) {
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

    pub(super) fn refresh_sidebar_subtitle(&self, session_uuid: &str) {
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
            workspace_connection_summary(endpoint, pane_info.as_deref())
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

    pub(super) fn refresh_sidebar_subtitle_if_active(&self, terminal_uuid: &str) {
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
}
