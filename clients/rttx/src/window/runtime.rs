use super::*;

impl Window {
    pub fn add_session(&self) {
        self.add_managed_session(WorkspacePolicy::Persistent);
    }

    pub(super) fn add_ephemeral_session(&self) {
        self.add_managed_session(WorkspacePolicy::Ephemeral);
    }

    pub(super) fn show_new_remote_workspace_dialog(&self) {
        let dialog =
            adw::Dialog::builder().title("New Remote Workspace").content_width(440).build();
        let header = adw::HeaderBar::new();
        let create_button = gtk4::Button::with_label("Create");
        create_button.add_css_class("suggested-action");
        header.pack_end(&create_button);

        let host_row = adw::EntryRow::builder().title("SSH host (e.g. user@host)").build();

        let status_label = gtk4::Label::new(None);
        status_label.set_xalign(0.0);
        status_label.add_css_class("dim-label");

        let group = adw::PreferencesGroup::new();
        group.add(&host_row);

        let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        content_box.set_margin_start(18);
        content_box.set_margin_end(18);
        content_box.set_margin_top(18);
        content_box.set_margin_bottom(18);
        content_box.append(&group);
        content_box.append(&status_label);

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&header);
        toolbar_view.set_content(Some(&content_box));
        dialog.set_child(Some(&toolbar_view));

        let dialog_ref = dialog.clone();
        let win = self.clone();
        create_button.connect_clicked(move |_| {
            let host = host_row.text().trim().to_string();
            if host.is_empty() {
                status_label.set_text("SSH host is required");
                return;
            }
            win.add_remote_managed_session(&host);
            dialog_ref.close();
        });

        dialog.present(Some(self));
    }

    pub(super) fn show_browse_remote_runtimes_dialog(&self) {
        let dialog =
            adw::Dialog::builder().title("Attach to Remote Runtime").content_width(440).build();
        let header = adw::HeaderBar::new();
        let connect_button = gtk4::Button::with_label("Connect");
        connect_button.add_css_class("suggested-action");
        header.pack_end(&connect_button);

        let host_row = adw::EntryRow::builder().title("SSH host (e.g. user@host)").build();

        let status_label = gtk4::Label::new(None);
        status_label.set_xalign(0.0);
        status_label.add_css_class("dim-label");
        status_label.set_text("Existing runtimes on the host will appear in the sidebar.");

        let group = adw::PreferencesGroup::new();
        group.add(&host_row);

        let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        content_box.set_margin_start(18);
        content_box.set_margin_end(18);
        content_box.set_margin_top(18);
        content_box.set_margin_bottom(18);
        content_box.append(&group);
        content_box.append(&status_label);

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&header);
        toolbar_view.set_content(Some(&content_box));
        dialog.set_child(Some(&toolbar_view));

        let dialog_ref = dialog.clone();
        let win = self.clone();
        connect_button.connect_clicked(move |_| {
            let host = host_row.text().trim().to_string();
            if host.is_empty() {
                status_label.set_text("SSH host is required");
                return;
            }
            win.browse_remote_runtimes(&host);
            dialog_ref.close();
        });

        dialog.present(Some(self));
    }

    fn browse_remote_runtimes(&self, host: &str) {
        if !self.ensure_connection_manager() {
            return;
        }
        let endpoint = RuntimeEndpoint::Remote { host: host.into() };
        if let Some(manager) = self.imp().connection_manager.borrow().as_ref() {
            manager.refresh_inventory(&endpoint);
        }
        self.show_toast(&format!("Connecting to {host}…"));
    }

    fn add_remote_managed_session(&self, host: &str) {
        let imp = self.imp();
        let count = imp.state.borrow().sessions.len() + 1;
        let endpoint = RuntimeEndpoint::Remote { host: host.to_string() };
        let name = crate::session::state::workspace_display_name(&endpoint, None, count);
        let mut session_state =
            SessionState::new_managed_remote(name, host, WorkspacePolicy::Persistent, None);
        session_state.color = self.next_session_color();
        imp.state.borrow_mut().sessions.push(session_state.clone());
        self.build_session(&session_state, false);
        self.set_workspace_connection_status(&session_state.uuid, &ConnectionStatus::Connecting);
        self.connect_managed_workspace(&session_state);

        let index = imp.state.borrow().sessions.len() as i32 - 1;
        if let Some(row) = imp.sidebar_list.row_at_index(index) {
            imp.sidebar_list.select_row(Some(&row));
        }
    }

    pub(super) fn add_managed_session(&self, policy: WorkspacePolicy) {
        let imp = self.imp();
        let count = imp.state.borrow().sessions.len() + 1;
        let initial_cwd = self.resolve_default_session_folder();
        let name = crate::session::state::workspace_display_name(
            &RuntimeEndpoint::Local,
            initial_cwd.as_deref(),
            count,
        );
        let mut session_state = SessionState::new_managed_local(name, policy, initial_cwd);
        session_state.color = self.next_session_color();
        imp.state.borrow_mut().sessions.push(session_state.clone());
        self.build_session(&session_state, false);
        self.set_workspace_connection_status(&session_state.uuid, &ConnectionStatus::Connecting);
        self.connect_managed_workspace(&session_state);

        let index = imp.state.borrow().sessions.len() as i32 - 1;
        if let Some(row) = imp.sidebar_list.row_at_index(index) {
            imp.sidebar_list.select_row(Some(&row));
        }
    }

    pub(super) fn ensure_connection_manager(&self) -> bool {
        if self.imp().connection_manager.borrow().is_some() {
            return true;
        }

        match crate::daemon_bridge::EndpointConnectionManager::new() {
            Ok((manager, rx)) => {
                self.start_endpoint_event_poller(rx);
                self.imp().connection_manager.replace(Some(manager));
                true
            }
            Err(error) => {
                log::error!("Failed to create endpoint connection manager: {error}");
                self.show_toast("Failed to initialize runtime connection manager");
                false
            }
        }
    }

    pub(super) fn connect_managed_workspace(&self, session_state: &SessionState) {
        if !session_state.uses_managed_runtime() || !self.ensure_connection_manager() {
            return;
        }

        let placeholder_terminal_uuid = session_state.layout.terminal_uuids().into_iter().next();
        if let Some(manager) = self.imp().connection_manager.borrow().as_ref() {
            manager.open_workspace(
                &session_state.uuid,
                &session_state.runtime.endpoint,
                &session_state.name,
                session_state.runtime.policy,
                session_state.runtime.runtime_id.as_deref(),
                placeholder_terminal_uuid.as_deref(),
            );
        }
    }

    pub(super) fn connect_managed_pane(
        &self,
        session_state: &SessionState,
        pane_view: &PersistentPaneView,
    ) {
        let terminal_uuid = pane_view.uuid();
        let status = self
            .imp()
            .workspace_connection_status
            .borrow()
            .get(&session_state.uuid)
            .cloned()
            .unwrap_or(ConnectionStatus::Connecting);
        let presentation = self.connection_presentation_for_workspace(&status);
        pane_view.set_connection_presentation(&status, &presentation);

        let win = self.clone();
        let input_terminal_uuid = terminal_uuid.clone();
        pane_view.connect_input(move |bytes| {
            win.send_managed_terminal_input(&input_terminal_uuid, bytes.to_vec());
        });

        let win = self.clone();
        let resize_terminal_uuid = terminal_uuid.clone();
        pane_view.connect_resize(move |cols, rows| {
            win.send_managed_terminal_resize(&resize_terminal_uuid, cols, rows);
        });

        let win = self.clone();
        let focus_uuid = terminal_uuid.clone();
        let focus_controller = gtk4::EventControllerFocus::new();
        focus_controller.connect_enter(move |_| {
            win.set_focused_terminal(Some(&focus_uuid));
        });
        pane_view.vte().add_controller(focus_controller);

        let win = self.clone();
        let split_h_uuid = terminal_uuid.clone();
        pane_view.split_h_button().connect_clicked(move |_| {
            win.split_terminal(&split_h_uuid, SplitOrientation::Horizontal);
        });

        let win = self.clone();
        let split_v_uuid = terminal_uuid.clone();
        pane_view.split_v_button().connect_clicked(move |_| {
            win.split_terminal(&split_v_uuid, SplitOrientation::Vertical);
        });

        let win = self.clone();
        let close_uuid = terminal_uuid;
        pane_view.close_button().connect_clicked(move |_| {
            win.close_terminal(&close_uuid);
        });

        let bell_pane = pane_view.clone();
        pane_view.vte().connect_bell(move |_| {
            bell_pane.flash_bell();
        });
    }

    pub(super) fn retry_workspace_connection(&self, workspace_id: &str) {
        let session_state = {
            let state = self.imp().state.borrow();
            state.sessions.iter().find(|session| session.uuid == workspace_id).cloned()
        };
        let Some(session_state) = session_state else {
            return;
        };
        self.set_workspace_connection_status(workspace_id, &ConnectionStatus::Connecting);
        self.connect_managed_workspace(&session_state);
    }

    pub(super) fn send_managed_terminal_input(&self, terminal_uuid: &str, data: Vec<u8>) {
        let Some((workspace_id, endpoint, runtime_id, runtime_pane_id)) =
            self.managed_binding_for_terminal(terminal_uuid)
        else {
            return;
        };
        if let Some(manager) = self.imp().connection_manager.borrow().as_ref() {
            manager.send_input(&workspace_id, &endpoint, &runtime_id, &runtime_pane_id, data);
        }
    }

    pub(super) fn send_managed_terminal_resize(&self, terminal_uuid: &str, cols: u16, rows: u16) {
        let Some((workspace_id, endpoint, runtime_id, runtime_pane_id)) =
            self.managed_binding_for_terminal(terminal_uuid)
        else {
            return;
        };
        if let Some(manager) = self.imp().connection_manager.borrow().as_ref() {
            manager.resize_pane(
                &workspace_id,
                &endpoint,
                &runtime_id,
                &runtime_pane_id,
                cols,
                rows,
            );
        }
    }

    pub(super) fn managed_binding_for_terminal(
        &self,
        terminal_uuid: &str,
    ) -> Option<(String, RuntimeEndpoint, String, String)> {
        let state = self.imp().state.borrow();
        state.managed_terminal_binding(terminal_uuid).map(|binding| {
            (binding.workspace_id, binding.endpoint, binding.runtime_id, binding.runtime_pane_id)
        })
    }

    pub(super) fn start_endpoint_event_poller(
        &self,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<crate::daemon_bridge::EndpointEvent>,
    ) {
        let win = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(8), move || {
            while let Ok(event) = rx.try_recv() {
                win.handle_endpoint_event(event);
            }
            glib::ControlFlow::Continue
        });
    }

    pub(super) fn handle_endpoint_event(&self, event: crate::daemon_bridge::EndpointEvent) {
        use crate::daemon_bridge::EndpointEvent;

        match event {
            EndpointEvent::RuntimeMessage { endpoint, message } => {
                self.dispatch_managed_runtime_message(&endpoint, &message);
            }
            EndpointEvent::WorkspaceError { workspace_id, detail, .. } => {
                log::warn!("Workspace {workspace_id} runtime error: {detail}");
                if !workspace_id.starts_with("inventory:") {
                    self.show_toast(&detail);
                }
            }
            other => {
                let transition = {
                    let mut state = self.imp().state.borrow_mut();
                    state.reconcile_endpoint_event(&other)
                };
                self.apply_endpoint_event_transition(&transition);
            }
        }
    }

    pub(super) fn apply_endpoint_event_transition(&self, transition: &EndpointEventTransition) {
        for session_state in &transition.recovered_workspaces {
            self.build_session(session_state, false);
        }

        for runtime_pane_id in &transition.skipped_runtime_panes {
            log::warn!("Failed to recover runtime pane {runtime_pane_id}: split depth limit");
        }

        for layout_terminal_uuid in &transition.removed_layout_terminals {
            self.imp().persistent_terminals.borrow_mut().remove(layout_terminal_uuid);
        }

        for rebuild in &transition.rebuilt_workspaces {
            self.rebuild_session_content(&rebuild.workspace_id, &rebuild.session_state);
        }

        if let Some(manager) = self.imp().connection_manager.borrow().as_ref() {
            for request in &transition.pane_create_requests {
                manager.create_pane(
                    &request.workspace_id,
                    &request.endpoint,
                    &request.runtime_id,
                    &request.layout_terminal_uuid,
                    request.cwd.clone(),
                    adw::StyleManager::default().is_dark(),
                );
            }
        }

        for restore in &transition.pane_snapshot_restores {
            self.restore_managed_snapshot(restore);
        }

        for layout_terminal_uuid in &transition.connected_layout_terminals {
            self.mark_managed_pane_connected(layout_terminal_uuid);
        }

        for layout_terminal_uuid in &transition.layout_terminals_to_recover {
            self.trigger_managed_recovery_for_terminal(layout_terminal_uuid);
        }

        for status_update in &transition.connection_status_updates {
            self.set_workspace_connection_status(
                &status_update.workspace_id,
                &status_update.status,
            );
        }

        for session_state in &transition.recovered_workspaces {
            self.connect_managed_workspace(session_state);
        }

        if transition.persist_window_state {
            self.save_state();
        }

        if !transition.rebuilt_workspaces.is_empty() || !transition.recovered_workspaces.is_empty()
        {
            self.sync_sidebar_to_visible_session();
        }
    }

    pub(super) fn restore_managed_snapshot(&self, restore: &WorkspacePaneRestore) {
        let pane = {
            let panes = self.imp().persistent_terminals.borrow();
            panes.get(&restore.layout_terminal_uuid).cloned()
        };
        let Some(pane) = pane else { return };

        pane.vte().reset(true, true);
        pane.feed_snapshot(&restore.scrollback);
        pane.set_current_directory(Some(&restore.cwd));
        if !restore.title.is_empty() && pane.custom_title().is_none() {
            pane.set_title(&restore.title);
        }
        pane.set_connected(true);

        let (cols, rows) = pane.terminal_size();
        if cols > 0 && rows > 0 {
            self.send_managed_terminal_resize(&restore.layout_terminal_uuid, cols, rows);
        }
    }

    pub(super) fn mark_managed_pane_connected(&self, layout_terminal_uuid: &str) {
        if let Some(pane) =
            self.imp().persistent_terminals.borrow().get(layout_terminal_uuid).cloned()
        {
            pane.set_connected(true);
            let (cols, rows) = pane.terminal_size();
            if cols > 0 && rows > 0 {
                self.send_managed_terminal_resize(layout_terminal_uuid, cols, rows);
            }
        }
    }

    pub(super) fn replace_workspace_connection_status(
        &self,
        workspace_id: &str,
        status: &ConnectionStatus,
    ) {
        self.imp()
            .workspace_connection_status
            .borrow_mut()
            .insert(workspace_id.to_string(), status.clone());
        self.refresh_workspace_row_status(workspace_id, status);
        self.refresh_workspace_pane_statuses(workspace_id, status);
    }

    pub(super) fn clear_workspace_reconnect_countdown(&self, workspace_id: &str) {
        if let Some(source_id) =
            self.imp().workspace_reconnect_sources.borrow_mut().remove(workspace_id)
        {
            source_id.remove();
        }
    }

    pub(super) fn start_workspace_reconnect_countdown(
        &self,
        workspace_id: &str,
        attempt: u32,
        retry_in_secs: u32,
    ) {
        if retry_in_secs <= 1 {
            return;
        }

        let win = self.clone();
        let workspace_id = workspace_id.to_string();
        let timer_workspace_id = workspace_id.clone();
        let mut remaining = retry_in_secs;
        let source_id = glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
            let current =
                win.imp().workspace_connection_status.borrow().get(&timer_workspace_id).cloned();
            let Some(ConnectionStatus::Reconnecting { attempt: current_attempt, .. }) = current
            else {
                win.imp().workspace_reconnect_sources.borrow_mut().remove(&timer_workspace_id);
                return glib::ControlFlow::Break;
            };
            if current_attempt != attempt {
                win.imp().workspace_reconnect_sources.borrow_mut().remove(&timer_workspace_id);
                return glib::ControlFlow::Break;
            }
            if remaining <= 1 {
                win.imp().workspace_reconnect_sources.borrow_mut().remove(&timer_workspace_id);
                return glib::ControlFlow::Break;
            }

            remaining -= 1;
            win.replace_workspace_connection_status(
                &timer_workspace_id,
                &ConnectionStatus::Reconnecting { attempt, retry_in_secs: remaining },
            );

            if remaining > 1 {
                glib::ControlFlow::Continue
            } else {
                win.imp().workspace_reconnect_sources.borrow_mut().remove(&timer_workspace_id);
                glib::ControlFlow::Break
            }
        });
        self.imp().workspace_reconnect_sources.borrow_mut().insert(workspace_id, source_id);
    }

    pub(super) fn set_workspace_connection_status(
        &self,
        workspace_id: &str,
        status: &ConnectionStatus,
    ) {
        self.clear_workspace_reconnect_countdown(workspace_id);
        self.replace_workspace_connection_status(workspace_id, status);
        if let ConnectionStatus::Reconnecting { attempt, retry_in_secs } = status {
            self.start_workspace_reconnect_countdown(workspace_id, *attempt, *retry_in_secs);
        }
    }

    pub(super) fn connection_presentation_for_workspace(
        &self,
        status: &ConnectionStatus,
    ) -> ConnectionPresentation {
        present_connection_status(status)
    }

    pub(super) fn refresh_workspace_row_status(
        &self,
        workspace_id: &str,
        status: &ConnectionStatus,
    ) {
        self.refresh_sidebar_subtitle(workspace_id);

        let state = self.imp().state.borrow();
        let Some(session) = state.sessions.iter().find(|session| session.uuid == workspace_id)
        else {
            return;
        };
        let icon = connection_icon(&session.runtime.endpoint, status);
        drop(state);

        let list = &self.imp().sidebar_list;
        let mut idx = 0;
        while let Some(row) = list.row_at_index(idx) {
            if let Some(session_row) =
                row.child().and_then(|child| child.downcast::<SessionRow>().ok())
                && session_row.uuid() == workspace_id
            {
                session_row.set_connection_icon(&icon);
                break;
            }
            idx += 1;
        }
    }

    pub(super) fn refresh_workspace_pane_statuses(
        &self,
        workspace_id: &str,
        status: &ConnectionStatus,
    ) {
        let terminal_uuids = {
            let state = self.imp().state.borrow();
            let Some(session) = state.sessions.iter().find(|session| session.uuid == workspace_id)
            else {
                return;
            };
            session.layout.terminal_uuids()
        };
        let presentation = self.connection_presentation_for_workspace(status);

        let panes = self.imp().persistent_terminals.borrow();
        for terminal_uuid in terminal_uuids {
            if let Some(pane) = panes.get(&terminal_uuid) {
                pane.set_connection_presentation(status, &presentation);
            }
        }
    }

    pub(super) fn dispatch_managed_runtime_message(
        &self,
        endpoint: &RuntimeEndpoint,
        msg: &rttx_proto::proto::ServerMessage,
    ) {
        use rttx_proto::proto::server_message::Msg;

        let Some(inner) = msg.msg.as_ref() else {
            return;
        };

        if let Msg::Error(error) = inner {
            log::error!("Daemon error: {} (code {})", error.message, error.code);
            return;
        }

        if let Msg::SessionTerminated(terminated) = inner {
            let Ok(runtime_id) = rttx_proto::bytes_to_uuid(&terminated.session_id) else {
                return;
            };
            let runtime_id = runtime_id.to_string();
            let workspace_id = {
                let state = self.imp().state.borrow();
                state.workspace_for_runtime(endpoint, &runtime_id)
            };
            if let Some(workspace_id) = workspace_id {
                self.set_workspace_connection_status(
                    &workspace_id,
                    &ConnectionStatus::Disconnected,
                );
            }
            return;
        }

        let Some(pane_id) = crate::daemon::extract_pane_id(msg) else {
            return;
        };
        let runtime_pane_id = pane_id.to_string();
        let (workspace_id, layout_terminal_uuid) = {
            let state = self.imp().state.borrow();
            let Some(target) = state.runtime_pane_target(endpoint, &runtime_pane_id) else {
                return;
            };
            target
        };

        let pane = {
            let panes = self.imp().persistent_terminals.borrow();
            panes.get(&layout_terminal_uuid).cloned()
        };
        let Some(pane) = pane else { return };

        match inner {
            Msg::Delta(delta) => {
                pane.feed_output(&delta.data);
                self.mark_session_activity(&layout_terminal_uuid);
            }
            Msg::TitleChanged(title_changed) => {
                if pane.custom_title().is_none() {
                    pane.set_title(&title_changed.title);
                }
                self.refresh_sidebar_subtitle_if_active(&layout_terminal_uuid);
            }
            Msg::CwdChanged(cwd_changed) => {
                pane.set_current_directory(Some(&cwd_changed.cwd));
                self.maybe_auto_rename_workspace(&workspace_id, Some(&cwd_changed.cwd));
                self.refresh_sidebar_subtitle_if_active(&layout_terminal_uuid);
            }
            Msg::PaneExited(exited) => {
                let visible_session = self.imp().session_stack.visible_child_name();
                let state = self.imp().state.borrow();
                let in_background = terminal_is_in_background_session(
                    &layout_terminal_uuid,
                    visible_session.as_deref(),
                    &state,
                );
                drop(state);
                if in_background {
                    self.notify_process_completed(&layout_terminal_uuid, exited.status);
                }
            }
            Msg::Bell(_) => pane.flash_bell(),
            Msg::PaneResized(_)
            | Msg::PaneCreated(_)
            | Msg::PaneClosed(_)
            | Msg::HelloAck(_)
            | Msg::SessionList(_)
            | Msg::SessionCreated(_)
            | Msg::Snapshot(_)
            | Msg::AttachBlocked(_)
            | Msg::SessionDetached(_)
            | Msg::SessionTerminated(_)
            | Msg::Pong(_)
            | Msg::Error(_) => {}
        }

        let _ = workspace_id;
    }

    pub(super) fn workspace_action_presentation(
        &self,
        session_uuid: &str,
    ) -> Option<WorkspaceActionPresentation> {
        let state = self.imp().state.borrow();
        let session = state.sessions.iter().find(|s| s.uuid == session_uuid)?;
        let policy = session.uses_managed_runtime().then_some(session.runtime.policy);
        let runtime_attached = session.runtime.runtime_id.is_some();
        Some(present_workspace_actions(policy, runtime_attached, session.layout.terminal_count()))
    }

    pub(super) fn show_edit_workspace_connection_dialog(&self, workspace_id: &str) {
        let current_host = {
            let state = self.imp().state.borrow();
            let Some(session) = state.sessions.iter().find(|session| session.uuid == workspace_id)
            else {
                return;
            };
            let RuntimeEndpoint::Remote { host } = &session.runtime.endpoint else {
                return;
            };
            host.clone()
        };

        let dialog = adw::Dialog::builder().title("Edit Connection").content_width(440).build();
        let header = adw::HeaderBar::new();
        let save_button = gtk4::Button::with_label("Save");
        save_button.add_css_class("suggested-action");
        header.pack_end(&save_button);

        let host_row = adw::EntryRow::builder().title("SSH target / args").build();
        host_row.set_text(&current_host);

        let status_label = gtk4::Label::new(None);
        status_label.set_xalign(0.0);
        status_label.add_css_class("dim-label");

        let group = adw::PreferencesGroup::new();
        group.add(&host_row);

        let content_box = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        content_box.set_margin_start(18);
        content_box.set_margin_end(18);
        content_box.set_margin_top(18);
        content_box.set_margin_bottom(18);
        content_box.append(&group);
        content_box.append(&status_label);

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&header);
        toolbar_view.set_content(Some(&content_box));
        dialog.set_child(Some(&toolbar_view));

        let dialog_for_save = dialog.clone();
        let win = self.clone();
        let workspace_id = workspace_id.to_string();
        save_button.connect_clicked(move |_| {
            let host = host_row.text().trim().to_string();
            if host.is_empty() {
                status_label.set_text("SSH target / args is required");
                return;
            }
            win.update_workspace_endpoint(&workspace_id, host);
            dialog_for_save.close();
        });

        dialog.present(Some(self));
    }

    pub(super) fn update_workspace_endpoint(&self, workspace_id: &str, host: String) {
        let (session_state, previous_endpoint) = {
            let mut state = self.imp().state.borrow_mut();
            let Some(session) =
                state.sessions.iter_mut().find(|session| session.uuid == workspace_id)
            else {
                return;
            };
            let previous_endpoint = session.runtime.endpoint.clone();
            session.runtime.endpoint = RuntimeEndpoint::Remote { host };
            session.sync_legacy_mode_from_runtime();
            (session.clone(), previous_endpoint)
        };

        if let Some(manager) = self.imp().connection_manager.borrow().as_ref() {
            manager.forget_workspace(&previous_endpoint, workspace_id);
        }

        self.set_workspace_connection_status(workspace_id, &ConnectionStatus::Connecting);
        self.connect_managed_workspace(&session_state);
    }

    pub(super) fn maybe_auto_rename_workspace(&self, workspace_id: &str, cwd: Option<&str>) {
        let mut state = self.imp().state.borrow_mut();
        let Some(session) = state.sessions.iter_mut().find(|s| s.uuid == workspace_id) else {
            return;
        };
        if session.user_renamed {
            return;
        }
        let Some(new_name) =
            crate::session::state::auto_name_for_workspace(&session.runtime.endpoint, cwd)
        else {
            return;
        };
        if session.name == new_name {
            return;
        }
        session.name.clone_from(&new_name);
        drop(state);
        self.update_sidebar_row_name(workspace_id, &new_name);
    }

    fn update_sidebar_row_name(&self, session_uuid: &str, name: &str) {
        let mut idx = 0;
        while let Some(row) = self.imp().sidebar_list.row_at_index(idx) {
            if let Some(session_row) =
                row.child().and_then(|child| child.downcast::<SessionRow>().ok())
                && session_row.uuid() == session_uuid
            {
                session_row.set_session_name(name);
                return;
            }
            idx += 1;
        }
    }
}
