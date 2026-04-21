use super::*;
use rttx_proto::v3;

/// Maximum events the poller processes per GTK timer callback.
/// Keeps the main loop responsive during output bursts.
pub(super) const EVENT_POLL_BATCH_LIMIT: usize = 64;

impl Window {
    pub fn add_session(&self) {
        crate::new_workspace_dialog::show(self, &crate::host::Host::local());
    }

    pub(super) fn add_ephemeral_session(&self) {
        self.add_managed_session(WorkspacePolicy::Ephemeral);
    }

    pub(crate) fn add_direct_session(&self) {
        let imp = self.imp();
        let count = imp.state.borrow().workspaces.len() + 1;
        let initial_cwd = self.resolve_default_session_folder();
        let name = format!("Direct {count}");
        let mut session_state = WorkspaceState::new_with_initial_cwd(name, initial_cwd);
        session_state.color = self.next_session_color();
        imp.state.borrow_mut().workspaces.push(session_state.clone());
        self.build_session(&session_state, false);

        let index = imp.state.borrow().workspaces.len() as i32 - 1;
        if let Some(row) = imp.sidebar_list.row_at_index(index) {
            imp.sidebar_list.select_row(Some(&row));
        }
    }

    pub(super) fn show_new_remote_workspace_dialog(&self) {
        let dialog =
            adw::Dialog::builder().title("New Remote Workspace").content_width(440).build();
        let header = adw::HeaderBar::new();
        let create_button = gtk4::Button::with_label("Next");
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
            let host_text = host_row.text().trim().to_string();
            if host_text.is_empty() {
                status_label.set_text("SSH host is required");
                return;
            }
            dialog_ref.close();
            let host = crate::host::Host::remote(&host_text);
            crate::new_workspace_dialog::show(&win, &host);
        });

        dialog.present(Some(self));
    }

    pub(super) fn show_browse_remote_runtimes_dialog(&self) {
        let dialog = adw::Dialog::builder().title("Connect to Existing").content_width(440).build();
        let header = adw::HeaderBar::new();
        let connect_button = gtk4::Button::with_label("Connect");
        connect_button.add_css_class("suggested-action");
        header.pack_end(&connect_button);

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
        connect_button.connect_clicked(move |_| {
            let host_text = host_row.text().trim().to_string();
            if host_text.is_empty() {
                status_label.set_text("SSH host is required");
                return;
            }
            dialog_ref.close();
            let host = crate::host::Host::remote(&host_text);
            win.request_connect_existing(&host);
        });

        dialog.present(Some(self));
    }

    /// Initiate the Connect to Existing flow for a local host.
    pub(super) fn connect_existing_local(&self) {
        self.request_connect_existing(&crate::host::Host::local());
    }

    /// Request session inventory for a host and show the Connect to Existing
    /// dialog when the response arrives.
    pub(super) fn request_connect_existing(&self, host: &crate::host::Host) {
        if !self.ensure_connection_manager() {
            return;
        }
        let endpoint = if host.is_local() {
            RuntimeEndpoint::Local
        } else {
            RuntimeEndpoint::Remote { host: host.ssh_target.clone().unwrap_or_default() }
        };
        self.imp().pending_connect_existing.replace(Some(host.clone()));
        if let Some(manager) = self.imp().connection_manager.borrow().as_ref() {
            manager.refresh_inventory(&endpoint);
        }
    }

    /// Return runtime IDs already attached by this client for a given host.
    pub(crate) fn open_runtime_ids_for_endpoint(&self, host: &crate::host::Host) -> Vec<String> {
        let endpoint = if host.is_local() {
            RuntimeEndpoint::Local
        } else {
            RuntimeEndpoint::Remote { host: host.ssh_target.clone().unwrap_or_default() }
        };
        let state = self.imp().state.borrow();
        state
            .workspaces
            .iter()
            .filter(|s| s.uses_managed_runtime() && s.runtime.endpoint == endpoint)
            .filter_map(|s| s.runtime.runtime_id.clone())
            .collect()
    }

    /// Attach to an existing runtime on a host by creating a new workspace
    /// bound to that runtime ID.
    pub(crate) fn attach_to_existing_runtime(&self, host: &crate::host::Host, runtime_id: &str) {
        let imp = self.imp();
        let count = imp.state.borrow().workspaces.len() + 1;
        let name = format!("Workspace {count}");

        let mut session_state = if host.is_local() {
            WorkspaceState::new_managed_local(name, WorkspacePolicy::Persistent, None)
        } else {
            let ssh_target = host.ssh_target.as_deref().unwrap_or(&host.key);
            WorkspaceState::new_managed_remote(name, ssh_target, WorkspacePolicy::Persistent, None)
        };
        session_state.runtime.runtime_id = Some(runtime_id.to_string());
        session_state.color = self.next_session_color();
        imp.state.borrow_mut().workspaces.push(session_state.clone());
        self.build_session(&session_state, false);
        self.set_workspace_connection_status(&session_state.uuid, &ConnectionStatus::Connecting);
        self.connect_managed_workspace(&session_state);

        let index = imp.state.borrow().workspaces.len() as i32 - 1;
        if let Some(row) = imp.sidebar_list.row_at_index(index) {
            imp.sidebar_list.select_row(Some(&row));
        }
    }

    /// Create a new remote managed workspace at a specific path.
    pub(crate) fn add_remote_managed_session_at(&self, host: &str, initial_cwd: Option<String>) {
        let imp = self.imp();
        let count = imp.state.borrow().workspaces.len() + 1;
        let endpoint = RuntimeEndpoint::Remote { host: host.to_string() };
        let name = crate::workspace::state::workspace_display_name(
            &endpoint,
            initial_cwd.as_deref(),
            count,
        );
        let mut session_state = WorkspaceState::new_managed_remote(
            name,
            host,
            WorkspacePolicy::Persistent,
            initial_cwd,
        );
        session_state.color = self.next_session_color();
        imp.state.borrow_mut().workspaces.push(session_state.clone());
        self.build_session(&session_state, false);
        self.set_workspace_connection_status(&session_state.uuid, &ConnectionStatus::Connecting);
        self.connect_managed_workspace(&session_state);

        let index = imp.state.borrow().workspaces.len() as i32 - 1;
        if let Some(row) = imp.sidebar_list.row_at_index(index) {
            imp.sidebar_list.select_row(Some(&row));
        }
    }

    pub(super) fn add_managed_session(&self, policy: WorkspacePolicy) {
        self.add_managed_session_at_with_policy(policy, self.resolve_default_session_folder());
    }

    /// Create a new local managed workspace at a specific path.
    pub(crate) fn add_managed_session_at(&self, initial_cwd: Option<String>) {
        self.add_managed_session_at_with_policy(WorkspacePolicy::Persistent, initial_cwd);
    }

    fn add_managed_session_at_with_policy(
        &self,
        policy: WorkspacePolicy,
        initial_cwd: Option<String>,
    ) {
        let imp = self.imp();
        let count = imp.state.borrow().workspaces.len() + 1;
        let name = crate::workspace::state::workspace_display_name(
            &RuntimeEndpoint::Local,
            initial_cwd.as_deref(),
            count,
        );
        let mut session_state = WorkspaceState::new_managed_local(name, policy, initial_cwd);
        session_state.color = self.next_session_color();
        imp.state.borrow_mut().workspaces.push(session_state.clone());
        self.build_session(&session_state, false);
        self.set_workspace_connection_status(&session_state.uuid, &ConnectionStatus::Connecting);
        self.connect_managed_workspace(&session_state);

        let index = imp.state.borrow().workspaces.len() as i32 - 1;
        if let Some(row) = imp.sidebar_list.row_at_index(index) {
            imp.sidebar_list.select_row(Some(&row));
        }
    }

    pub(super) fn ensure_connection_manager(&self) -> bool {
        if self.imp().connection_manager.borrow().is_some() {
            return true;
        }

        let prefs =
            crate::store::default_store().load_preferences().into_value().unwrap_or_default();
        match crate::daemon_bridge::EndpointConnectionManager::new(
            prefs.auto_start_daemon,
            prefs.reconnect_delay_secs,
        ) {
            Ok((manager, rx)) => {
                self.start_endpoint_event_poller(rx);
                self.imp().connection_manager.replace(Some(manager));
                true
            }
            Err(error) => {
                tracing::error!("Failed to create endpoint connection manager: {error}");
                self.show_toast("Failed to initialize runtime connection manager");
                false
            }
        }
    }

    pub(super) fn connect_managed_workspace(&self, session_state: &WorkspaceState) {
        if !session_state.uses_managed_runtime() || !self.ensure_connection_manager() {
            return;
        }

        let placeholder_terminal_uuid = session_state.layout.terminal_uuids().into_iter().next();
        let cwd = placeholder_terminal_uuid
            .as_deref()
            .and_then(|uuid| session_state.layout.terminal_cwd(uuid));
        if let Some(manager) = self.imp().connection_manager.borrow().as_ref() {
            manager.open_workspace(
                &session_state.uuid,
                &session_state.runtime.endpoint,
                &session_state.name,
                session_state.runtime.policy,
                session_state.runtime.runtime_id.as_deref(),
                placeholder_terminal_uuid.as_deref(),
                cwd.as_deref(),
            );
        }
    }

    pub(super) fn connect_managed_pane(
        &self,
        session_state: &WorkspaceState,
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

        let win = self.downgrade();
        let input_terminal_uuid = terminal_uuid.clone();
        pane_view.connect_input(move |bytes| {
            if let Some(win) = win.upgrade() {
                win.send_managed_terminal_input(&input_terminal_uuid, bytes);
            }
        });

        let win = self.downgrade();
        let resize_terminal_uuid = terminal_uuid.clone();
        pane_view.connect_resize(move |cols, rows| {
            if let Some(win) = win.upgrade() {
                win.send_managed_terminal_resize(&resize_terminal_uuid, cols, rows);
            }
        });

        let drag_source = gtk4::DragSource::new();
        drag_source.set_actions(gtk4::gdk::DragAction::MOVE);
        let drag_uuid = terminal_uuid.clone();
        drag_source.connect_prepare(move |_, _, _| {
            Some(gtk4::gdk::ContentProvider::for_value(&drag_uuid.to_value()))
        });
        pane_view.header().add_controller(drag_source);

        let drop_target = gtk4::DropTarget::new(glib::Type::STRING, gtk4::gdk::DragAction::MOVE);
        let win = self.downgrade();
        let target_uuid = terminal_uuid.clone();
        drop_target.connect_drop(move |_, value, _, _| {
            if let Ok(source_uuid) = value.get::<String>()
                && source_uuid != target_uuid
                && let Some(win) = win.upgrade()
            {
                win.swap_terminals(&source_uuid, &target_uuid);
                return true;
            }
            false
        });
        pane_view.add_controller(drop_target);

        let win = self.downgrade();
        let focus_uuid = terminal_uuid.clone();
        let focus_controller = gtk4::EventControllerFocus::new();
        focus_controller.connect_enter(move |_| {
            let Some(win) = win.upgrade() else { return };
            win.set_focused_terminal(Some(&focus_uuid));
            let session_uuid = {
                let mut state = win.imp().state.borrow_mut();
                let session = state
                    .workspaces
                    .iter_mut()
                    .find(|session| session.layout.contains_terminal(&focus_uuid));
                if let Some(session) = session {
                    session.active_terminal_uuid = Some(focus_uuid.clone());
                    Some(session.uuid.clone())
                } else {
                    None
                }
            };
            if let Some(session_uuid) = session_uuid {
                win.refresh_sidebar_subtitle(&session_uuid);
            }
        });
        pane_view.vte().add_controller(focus_controller);

        let win = self.downgrade();
        let split_h_uuid = terminal_uuid.clone();
        pane_view.split_h_button().connect_clicked(move |_| {
            if let Some(win) = win.upgrade() {
                win.split_terminal(&split_h_uuid, SplitOrientation::Horizontal);
            }
        });

        let win = self.downgrade();
        let split_v_uuid = terminal_uuid.clone();
        pane_view.split_v_button().connect_clicked(move |_| {
            if let Some(win) = win.upgrade() {
                win.split_terminal(&split_v_uuid, SplitOrientation::Vertical);
            }
        });

        let win = self.downgrade();
        let close_uuid = terminal_uuid;
        pane_view.close_button().connect_clicked(move |_| {
            if let Some(win) = win.upgrade() {
                win.close_terminal(&close_uuid);
            }
        });

        let win = self.downgrade();
        pane_view.zoom_button().connect_clicked(move |_| {
            if let Some(win) = win.upgrade() {
                win.toggle_pane_zoom();
            }
        });

        let bell_pane = pane_view.clone();
        pane_view.vte().connect_bell(move |_| {
            bell_pane.flash_bell();
        });
    }

    pub(super) fn retry_workspace_connection(&self, workspace_id: &str) {
        let session_state = {
            let state = self.imp().state.borrow();
            state.workspaces.iter().find(|session| session.uuid == workspace_id).cloned()
        };
        let Some(session_state) = session_state else {
            return;
        };
        self.set_workspace_connection_status(workspace_id, &ConnectionStatus::Connecting);
        if let Some(manager) = self.imp().connection_manager.borrow().as_ref() {
            manager.reset_endpoint(&session_state.runtime.endpoint);
        }
        self.connect_managed_workspace(&session_state);
    }

    pub(super) fn send_managed_terminal_input(&self, terminal_uuid: &str, data: &[u8]) {
        let (primary, sync_targets) = {
            let state = self.imp().state.borrow();
            (
                state
                    .managed_terminal_binding(terminal_uuid)
                    .map(|b| (b.workspace_id, b.endpoint, b.runtime_id, b.runtime_pane_id)),
                state.input_sync_targets(terminal_uuid),
            )
        };
        let Some((workspace_id, endpoint, runtime_id, runtime_pane_id)) = primary else {
            return;
        };
        if let Some(manager) = self.imp().connection_manager.borrow().as_ref() {
            let shared = bytes::Bytes::copy_from_slice(data);
            manager.send_input(
                &workspace_id,
                &endpoint,
                &runtime_id,
                &runtime_pane_id,
                shared.clone(),
            );
            for target in &sync_targets {
                manager.send_input(
                    &target.workspace_id,
                    &target.endpoint,
                    &target.runtime_id,
                    &target.runtime_pane_id,
                    shared.clone(),
                );
            }
        }
    }

    /// Send input to a single managed pane without input-sync fan-out.
    pub(super) fn send_managed_pane_input_direct(&self, terminal_uuid: &str, data: &[u8]) {
        let Some((workspace_id, endpoint, runtime_id, runtime_pane_id)) =
            self.managed_binding_for_terminal(terminal_uuid)
        else {
            return;
        };
        if let Some(manager) = self.imp().connection_manager.borrow().as_ref() {
            manager.send_input(
                &workspace_id,
                &endpoint,
                &runtime_id,
                &runtime_pane_id,
                bytes::Bytes::copy_from_slice(data),
            );
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
        mut rx: tokio::sync::mpsc::Receiver<crate::daemon_bridge::EndpointEvent>,
    ) {
        let win = self.downgrade();
        let source = glib::timeout_add_local(std::time::Duration::from_millis(8), move || {
            let Some(win) = win.upgrade() else {
                return glib::ControlFlow::Break;
            };
            for _ in 0..EVENT_POLL_BATCH_LIMIT {
                match rx.try_recv() {
                    Ok(event) => win.handle_endpoint_event(event),
                    Err(_) => break,
                }
            }
            glib::ControlFlow::Continue
        });
        self.imp().event_poller_source.replace(Some(source));
    }

    pub(super) fn handle_endpoint_event(&self, event: crate::daemon_bridge::EndpointEvent) {
        use crate::daemon_bridge::EndpointEvent;

        match event {
            EndpointEvent::RuntimeMessage { endpoint, message } => {
                self.dispatch_managed_runtime_message(&endpoint, &message);
            }
            EndpointEvent::WorkspaceError { workspace_id, detail, .. } => {
                tracing::warn!("Workspace {workspace_id} runtime error: {detail}");
                if !workspace_id.starts_with("inventory:") {
                    self.show_toast(&detail);
                }
            }
            EndpointEvent::InventoryLoaded { endpoint, runtimes }
                if self.imp().pending_connect_existing.borrow().is_some() =>
            {
                let host = self.imp().pending_connect_existing.take().unwrap();
                let expected_endpoint = if host.is_local() {
                    RuntimeEndpoint::Local
                } else {
                    RuntimeEndpoint::Remote { host: host.ssh_target.clone().unwrap_or_default() }
                };
                if endpoint == expected_endpoint {
                    crate::connect_existing_dialog::show(self, &host, &runtimes);
                } else {
                    // Wrong endpoint — put the pending request back and
                    // let the event go through normal reconciliation.
                    self.imp().pending_connect_existing.replace(Some(host));
                    let transition = {
                        let mut state = self.imp().state.borrow_mut();
                        state.reconcile_endpoint_event(&EndpointEvent::InventoryLoaded {
                            endpoint,
                            runtimes,
                        })
                    };
                    self.apply_endpoint_event_transition(&transition);
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
            tracing::warn!("Failed to recover runtime pane {runtime_pane_id}: split depth limit");
        }

        for layout_terminal_uuid in &transition.removed_layout_terminals {
            self.imp().persistent_terminals.borrow_mut().remove(layout_terminal_uuid);
        }

        for rebuild in &transition.rebuilt_workspaces {
            self.rebuild_session_content(&rebuild.workspace_id, &rebuild.session_state);
        }

        if let Some(manager) = self.imp().connection_manager.borrow().as_ref() {
            for request in &transition.pane_create_requests {
                let size = self.persistent_terminal_size(&request.layout_terminal_uuid);
                manager.create_pane(
                    &request.workspace_id,
                    &request.endpoint,
                    &request.runtime_id,
                    &request.layout_terminal_uuid,
                    request.cwd.clone(),
                    adw::StyleManager::default().is_dark(),
                    size,
                    false,
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
        pane.feed_snapshot(&restore.scrollback_tail);
        if let Some(ref modes) = restore.terminal_modes {
            pane.set_bracketed_paste_mode(modes.bracketed_paste);
            pane.restore_interaction_modes(
                modes.application_cursor_keys,
                modes.application_keypad,
                u32::from(rttx_proto::v3_terminal_modes::tracking_value_from_mouse_mode(
                    rttx_proto::v3::MouseMode::try_from(modes.mouse_mode)
                        .unwrap_or(rttx_proto::v3::MouseMode::None),
                )),
                modes.sgr_mouse,
            );
        }
        pane.set_current_directory(Some(&restore.cwd));
        if !restore.title.is_empty() && pane.custom_title().is_none() {
            pane.set_daemon_title(&restore.title);
        }
        pane.set_connected(true);

        let (cols, rows) = pane.terminal_size();
        if cols > 0 && rows > 0 && (cols != restore.cols || rows != restore.rows) {
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

        let win = self.downgrade();
        let workspace_id = workspace_id.to_string();
        let timer_workspace_id = workspace_id.clone();
        let mut remaining = retry_in_secs;
        let source_id = glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
            let Some(win) = win.upgrade() else {
                return glib::ControlFlow::Break;
            };
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
        let Some(session) = state.workspaces.iter().find(|session| session.uuid == workspace_id)
        else {
            return;
        };
        let icon =
            connection_icon(&session.runtime.endpoint, status, session.uses_managed_runtime());
        drop(state);

        let list = &self.imp().sidebar_list;
        let mut idx = 0;
        while let Some(row) = list.row_at_index(idx) {
            if let Some(session_row) =
                row.child().and_then(|child| child.downcast::<WorkspaceRow>().ok())
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
            let Some(session) =
                state.workspaces.iter().find(|session| session.uuid == workspace_id)
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
        msg: &v3::ServerEnvelope,
    ) {
        use v3::server_envelope::Payload;

        let Some(inner) = msg.payload.as_ref() else {
            return;
        };

        if let Payload::Error(error) = inner {
            tracing::warn!("Daemon error: {} ({:?})", error.message, error.kind());
            return;
        }

        if let Payload::RuntimeTerminated(terminated) = inner {
            let Ok(runtime_id) = rttx_proto::bytes_to_uuid(&terminated.runtime_id) else {
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
            Payload::OutputDelta(delta) => {
                pane.feed_output(&delta.data);
                self.mark_session_activity(&layout_terminal_uuid);
            }
            Payload::TitleChanged(t) => {
                if pane.custom_title().is_none() {
                    pane.set_daemon_title(&t.title);
                }
                self.refresh_sidebar_subtitle_if_active(&layout_terminal_uuid);
            }
            Payload::CwdChanged(c) => {
                pane.set_current_directory(Some(&c.cwd));
                {
                    let mut state = self.imp().state.borrow_mut();
                    if let Some(session) =
                        state.workspaces.iter_mut().find(|s| s.uuid == workspace_id)
                    {
                        session.layout.set_terminal_cwd(&layout_terminal_uuid, Some(c.cwd.clone()));
                    }
                }
                self.maybe_auto_rename_workspace(&workspace_id, Some(&c.cwd));
                self.refresh_sidebar_subtitle_if_active(&layout_terminal_uuid);
            }
            Payload::PaneExited(exited) => {
                pane.mark_exited(exited.status);
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
            Payload::Bell(_) => pane.flash_bell(),
            Payload::PaneResized(_)
            | Payload::PaneCreated(_)
            | Payload::PaneClosed(_)
            | Payload::RuntimeList(_)
            | Payload::RuntimeCreated(_)
            | Payload::RuntimeSnapshot(_)
            | Payload::AttachBlocked(_)
            | Payload::RuntimeDetached(_)
            | Payload::RuntimeTerminated(_)
            | Payload::RuntimeRenamed(_)
            | Payload::Pong(_)
            | Payload::Error(_)
            | Payload::DiagnosticsReport(_)
            | Payload::TerminalModeChanged(_)
            | Payload::StreamOverflow(_)
            | Payload::ScrollbackChunk(_)
            | Payload::TakeoverCompleted(_)
            | Payload::LeaseLost(_)
            | Payload::OwnerDisconnected(_) => {}
        }

        let _ = workspace_id;
    }

    pub(super) fn workspace_action_presentation(
        &self,
        session_uuid: &str,
    ) -> Option<WorkspaceActionPresentation> {
        let state = self.imp().state.borrow();
        let session = state.workspaces.iter().find(|s| s.uuid == session_uuid)?;
        let policy = session.uses_managed_runtime().then_some(session.runtime.policy);
        let runtime_attached = session.runtime.runtime_id.is_some();
        Some(present_workspace_actions(policy, runtime_attached, session.layout.terminal_count()))
    }

    pub(super) fn show_edit_workspace_connection_dialog(&self, workspace_id: &str) {
        let current_host = {
            let state = self.imp().state.borrow();
            let Some(session) =
                state.workspaces.iter().find(|session| session.uuid == workspace_id)
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
                state.workspaces.iter_mut().find(|session| session.uuid == workspace_id)
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
        let Some(session) = state.workspaces.iter_mut().find(|s| s.uuid == workspace_id) else {
            return;
        };
        if session.user_renamed {
            return;
        }
        let Some(new_name) =
            crate::workspace::state::auto_name_for_workspace(&session.runtime.endpoint, cwd)
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
                row.child().and_then(|child| child.downcast::<WorkspaceRow>().ok())
                && session_row.uuid() == session_uuid
            {
                session_row.set_workspace_name(name);
                return;
            }
            idx += 1;
        }
    }

    /// Return `(cols, rows)` from a persistent terminal's VTE widget.
    ///
    /// Returns `(0, 0)` when the terminal is not yet registered, letting the
    /// daemon fall back to its default size.
    pub(super) fn persistent_terminal_size(&self, layout_terminal_uuid: &str) -> (u32, u32) {
        let panes = self.imp().persistent_terminals.borrow();
        let Some(pane) = panes.get(layout_terminal_uuid) else {
            return (0, 0);
        };
        let (cols, rows) = pane.terminal_size();
        (u32::from(cols), u32::from(rows))
    }
}
