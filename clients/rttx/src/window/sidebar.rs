use super::*;

impl Window {
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

    pub(super) fn sidebar_row_for_uuid(&self, session_uuid: &str) -> Option<SessionRow> {
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
}
