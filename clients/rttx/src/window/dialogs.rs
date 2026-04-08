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

    pub(super) fn confirm_close_session(&self, session_uuid: &str) {
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

    pub(super) fn confirm_delete_bookmark(&self, uuid: String) {
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

    pub(super) fn confirm_delete_command(&self, uuid: String) {
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
        about.set_license_type(gtk4::License::Gpl30);
        about.present();
    }

    pub(super) fn show_sidebar_context_menu(&self, session_uuid: &str) {
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

    pub(super) fn show_rename_session_popover(&self, row: &SessionRow, session_uuid: &str) {
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

    pub(super) fn rename_session(&self, session_uuid: &str, new_name: &str) {
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

    pub(super) fn do_bookmark_active_session(&self) {
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
}
