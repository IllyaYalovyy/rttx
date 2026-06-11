use super::*;
use rttx_proto::v3;
use std::collections::BTreeMap;

/// Maximum events the poller processes per GTK timer callback.
/// Keeps the main loop responsive during output bursts.
pub(super) const EVENT_POLL_BATCH_LIMIT: usize = 64;

/// Interval between GTK event poller ticks.
pub(super) const EVENT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(8);

/// Time budget per poll tick. The poller breaks early when this is exceeded,
/// ensuring GTK gets at least half the interval for rendering and input.
pub(super) const EVENT_POLL_TIME_BUDGET: std::time::Duration = std::time::Duration::from_millis(4);

/// Result of partitioning a batch of endpoint events into coalesced output
/// deltas and remaining non-delta events.
pub(super) struct CoalescedBatch {
    /// Accumulated output bytes per pane, keyed by `(endpoint, pane_id_bytes)`.
    pub delta_buffers: BTreeMap<(RuntimeEndpoint, Vec<u8>), Vec<u8>>,
    /// Non-delta events in their original order.
    pub other_events: Vec<crate::daemon_bridge::EndpointEvent>,
}

/// Partition a batch of endpoint events: coalesce `OutputDelta` messages per
/// pane into a single buffer, and collect everything else in order.
pub(super) fn coalesce_event_batch(
    events: Vec<crate::daemon_bridge::EndpointEvent>,
) -> CoalescedBatch {
    let mut delta_buffers: BTreeMap<(RuntimeEndpoint, Vec<u8>), Vec<u8>> = BTreeMap::new();
    let mut other_events: Vec<crate::daemon_bridge::EndpointEvent> = Vec::new();

    for event in events {
        match event {
            crate::daemon_bridge::EndpointEvent::WorkspaceMessage { ref endpoint, ref message }
                if matches!(
                    message.payload,
                    Some(v3::server_envelope::Payload::OutputDelta(_))
                ) =>
            {
                if let Some(v3::server_envelope::Payload::OutputDelta(ref delta)) = message.payload
                {
                    let key = (endpoint.clone(), delta.pane_id.clone());
                    delta_buffers.entry(key).or_default().extend_from_slice(&delta.data);
                }
            }
            other => other_events.push(other),
        }
    }

    CoalescedBatch { delta_buffers, other_events }
}

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

    pub(super) fn show_browse_remote_workspaces_dialog(&self) {
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
            RuntimeEndpoint::remote_with_binary(
                host.ssh_target.clone().unwrap_or_default(),
                host.daemon_binary_path.clone(),
            )
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
            RuntimeEndpoint::remote_with_binary(
                host.ssh_target.clone().unwrap_or_default(),
                host.daemon_binary_path.clone(),
            )
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
        let endpoint = RuntimeEndpoint::remote(host);
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
            Ok((manager, rx, capacity_probe, backpressure)) => {
                self.start_endpoint_event_poller(rx, capacity_probe, backpressure);
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
        let zoom_uuid = terminal_uuid.clone();
        let close_uuid = terminal_uuid;
        pane_view.close_button().connect_clicked(move |_| {
            if let Some(win) = win.upgrade() {
                win.close_terminal(&close_uuid);
            }
        });

        let win = self.downgrade();
        pane_view.zoom_button().connect_clicked(move |_| {
            if let Some(win) = win.upgrade() {
                win.toggle_pane_zoom_for(Some(&zoom_uuid));
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

    /// Restart the local daemon and reconnect all workspaces on the local endpoint.
    pub(super) fn restart_daemon_and_reconnect(&self, workspace_id: &str) {
        self.set_workspace_connection_status(workspace_id, &ConnectionStatus::Starting);
        self.retry_all_workspaces_for_endpoint(workspace_id);
    }

    /// Reconnect all managed workspaces sharing the same endpoint as the given workspace.
    pub(super) fn retry_all_workspaces_for_endpoint(&self, workspace_id: &str) {
        let targets: Vec<WorkspaceState> = {
            let state = self.imp().state.borrow();
            let Some(origin) = state.workspaces.iter().find(|s| s.uuid == workspace_id) else {
                return;
            };
            let endpoint_key = origin.runtime.endpoint.key();
            let statuses = self.imp().workspace_connection_status.borrow();
            state
                .workspaces
                .iter()
                .filter(|s| {
                    s.uses_managed_runtime()
                        && s.runtime.endpoint.key() == endpoint_key
                        && statuses.get(&s.uuid).is_some_and(|st| {
                            matches!(
                                st,
                                ConnectionStatus::Disconnected
                                    | ConnectionStatus::Reconnecting { .. }
                                    | ConnectionStatus::Blocked(_)
                                    | ConnectionStatus::Connecting
                                    | ConnectionStatus::Starting
                            )
                        })
                })
                .cloned()
                .collect()
        };
        if let Some(first) = targets.first()
            && let Some(manager) = self.imp().connection_manager.borrow().as_ref()
        {
            manager.reset_endpoint(&first.runtime.endpoint);
        }
        for session_state in &targets {
            self.set_workspace_connection_status(
                &session_state.uuid,
                &ConnectionStatus::Connecting,
            );
            self.connect_managed_workspace(session_state);
        }
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
        capacity_probe: tokio::sync::mpsc::Sender<crate::daemon_bridge::EndpointEvent>,
        backpressure: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        let win = self.downgrade();
        let source = glib::timeout_add_local(EVENT_POLL_INTERVAL, move || {
            let Some(win) = win.upgrade() else {
                return glib::ControlFlow::Break;
            };

            let start = std::time::Instant::now();
            let mut events = Vec::new();
            for _ in 0..EVENT_POLL_BATCH_LIMIT {
                if start.elapsed() > EVENT_POLL_TIME_BUDGET {
                    break;
                }
                match rx.try_recv() {
                    Ok(event) => events.push(event),
                    Err(_) => break,
                }
            }

            if !events.is_empty() {
                let batch = coalesce_event_batch(events);
                win.feed_coalesced_deltas(&batch.delta_buffers);
                for event in batch.other_events {
                    win.handle_endpoint_event(event);
                }
            }

            // Watermark-based backpressure: check how many slots are free.
            let free = capacity_probe.capacity();
            let is_paused = backpressure.load(std::sync::atomic::Ordering::Acquire);
            if !is_paused && free <= crate::daemon_bridge::BACKPRESSURE_HIGH_WATERMARK {
                tracing::warn!(free, "Event channel high watermark reached, pausing actors");
                backpressure.store(true, std::sync::atomic::Ordering::Release);
            } else if is_paused && free >= crate::daemon_bridge::BACKPRESSURE_LOW_WATERMARK {
                tracing::info!(free, "Event channel low watermark reached, resuming actors");
                backpressure.store(false, std::sync::atomic::Ordering::Release);
            }

            glib::ControlFlow::Continue
        });
        self.imp().event_poller_source.replace(Some(source));
    }

    /// Feed coalesced output delta buffers — one `vte.feed()` call per pane.
    fn feed_coalesced_deltas(&self, delta_buffers: &BTreeMap<(RuntimeEndpoint, Vec<u8>), Vec<u8>>) {
        for ((endpoint, pane_id_bytes), data) in delta_buffers {
            let Ok(pane_id) = rttx_proto::bytes_to_uuid(pane_id_bytes) else {
                continue;
            };
            let runtime_pane_id = pane_id.to_string();
            let layout_terminal_uuid = {
                let state = self.imp().state.borrow();
                state.runtime_pane_target(endpoint, &runtime_pane_id).map(|(_, uuid)| uuid)
            };
            let Some(layout_terminal_uuid) = layout_terminal_uuid else {
                continue;
            };

            let pane = {
                let panes = self.imp().persistent_terminals.borrow();
                panes.get(&layout_terminal_uuid).cloned()
            };
            let Some(pane) = pane else { continue };

            pane.feed_output(data);
            self.mark_session_activity(&layout_terminal_uuid);
        }
    }

    pub(super) fn handle_endpoint_event(&self, event: crate::daemon_bridge::EndpointEvent) {
        use crate::daemon_bridge::EndpointEvent;

        match event {
            EndpointEvent::WorkspaceMessage { endpoint, message } => {
                self.dispatch_managed_runtime_message(&endpoint, &message);
            }
            EndpointEvent::WorkspaceError { workspace_id, detail, .. } => {
                tracing::warn!("Workspace {workspace_id} runtime error: {detail}");
                if !workspace_id.starts_with("inventory:") {
                    self.show_toast(&detail);
                }
            }
            EndpointEvent::InventoryLoaded { endpoint, workspaces }
                if self.imp().pending_connect_existing.borrow().is_some() =>
            {
                let host = self.imp().pending_connect_existing.take().unwrap();
                let expected_endpoint = if host.is_local() {
                    RuntimeEndpoint::Local
                } else {
                    RuntimeEndpoint::remote_with_binary(
                        host.ssh_target.clone().unwrap_or_default(),
                        host.daemon_binary_path.clone(),
                    )
                };
                if endpoint == expected_endpoint {
                    crate::connect_existing_dialog::show(self, &host, &workspaces);
                } else {
                    // Wrong endpoint — put the pending request back and
                    // let the event go through normal reconciliation.
                    self.imp().pending_connect_existing.replace(Some(host));
                    let transition = {
                        let mut state = self.imp().state.borrow_mut();
                        state.reconcile_endpoint_event(&EndpointEvent::InventoryLoaded {
                            endpoint,
                            workspaces,
                        })
                    };
                    self.apply_endpoint_event_transition(&transition);
                }
            }
            EndpointEvent::WorkspaceResynced { .. } => {
                let transition = {
                    let mut state = self.imp().state.borrow_mut();
                    state.reconcile_endpoint_event(&event)
                };
                self.apply_endpoint_event_transition(&transition);
                if !transition.pane_snapshot_restores.is_empty() {
                    self.show_toast("Terminal resynced — some output may have been lost");
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

        // Set VTE's internal size to the current pane dimensions before
        // feeding the snapshot. The snapshot bytes contain escape sequences
        // rendered at the old width; by telling VTE the correct column count
        // first, it wraps lines at the right position instead of using the
        // stale snapshot dimensions.
        let (cols, rows) = pane.terminal_size();
        if cols > 0 && rows > 0 && (cols != restore.cols || rows != restore.rows) {
            pane.vte().set_size(cols.into(), rows.into());
        }

        // Temporarily block input forwarding during snapshot feed so that
        // CPR responses generated by VTE (from stale ESC[6n in scrollback)
        // are not forwarded to the daemon as terminal input.
        let was_accepting = pane.imp().accepts_input.get();
        pane.imp().accepts_input.set(false);
        pane.feed_snapshot(&restore.scrollback_tail);
        if !pane.is_crashed() {
            if let Some(ref modes) = restore.terminal_modes {
                pane.set_bracketed_paste_mode(modes.bracketed_paste);
                // alternate_screen is intentionally not restored: the snapshot
                // already contains the rendered screen content, and switching
                // VTE into alt-screen mode would discard it.
                pane.restore_interaction_modes(modes);
            } else {
                // No mode state available — feed cleanup bytes to ensure any
                // mouse tracking or other modes left by the scrollback data
                // are disabled. Without this, stale DECSET sequences in the
                // scrollback can leave VTE with mouse tracking on, causing
                // mouse clicks to print escape sequences instead of working.
                pane.vte().feed(crate::terminal::terminal_cleanup_bytes());
            }
        }
        pane.imp().accepts_input.set(was_accepting);
        pane.set_current_directory(Some(&restore.cwd));
        if !restore.title.is_empty() && pane.custom_title().is_none() {
            pane.set_daemon_title(&restore.title);
        }
        pane.set_connected(true);

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

        if let Some(session_row) = self.sidebar_workspace_row(workspace_id) {
            session_row.set_connection_icon(&icon);
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

        if let Payload::WorkspaceTerminated(terminated) = inner {
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
            Payload::TerminalModeChanged(m) => {
                if let Some(modes) = m.modes {
                    pane.set_application_modes(
                        modes.application_cursor_keys,
                        modes.application_keypad,
                    );
                }
            }
            // Tree-delta and viewport events are consumed by the
            // server-authoritative client view in RFC-031 Step 4 (#1001).
            Payload::PaneResized(_)
            | Payload::PaneCreated(_)
            | Payload::PaneClosed(_)
            | Payload::PaneSplit(_)
            | Payload::SplitResized(_)
            | Payload::FocusChanged(_)
            | Payload::WorkspaceList(_)
            | Payload::WorkspaceCreated(_)
            | Payload::WorkspaceSnapshot(_)
            | Payload::AttachBlocked(_)
            | Payload::WorkspaceDetached(_)
            | Payload::WorkspaceTerminated(_)
            | Payload::WorkspaceRenamed(_)
            | Payload::Pong(_)
            | Payload::Error(_)
            | Payload::DiagnosticsReport(_)
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
            let RuntimeEndpoint::Remote { host, .. } = &session.runtime.endpoint else {
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
            win.update_workspace_endpoint(&workspace_id, &host);
            dialog_for_save.close();
        });

        dialog.present(Some(self));
    }

    pub(super) fn update_workspace_endpoint(&self, workspace_id: &str, host: &str) {
        let (session_state, previous_endpoint) = {
            let mut state = self.imp().state.borrow_mut();
            let Some(session) =
                state.workspaces.iter_mut().find(|session| session.uuid == workspace_id)
            else {
                return;
            };
            let previous_endpoint = session.runtime.endpoint.clone();
            session.runtime.endpoint = RuntimeEndpoint::remote(host);
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
        if let Some(session_row) = self.sidebar_workspace_row(session_uuid) {
            session_row.set_workspace_name(name);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_bridge::EndpointEvent;
    use rttx_proto::{uuid_to_bytes, v3_snapshot::build_output_delta_envelope};
    use uuid::Uuid;

    fn delta_event(
        endpoint: RuntimeEndpoint,
        pane_id: Uuid,
        data: &[u8],
        seq: u64,
    ) -> EndpointEvent {
        EndpointEvent::WorkspaceMessage {
            endpoint,
            message: build_output_delta_envelope(
                Uuid::nil(),
                pane_id,
                bytes::Bytes::copy_from_slice(data),
                seq,
            ),
        }
    }

    fn title_event(endpoint: RuntimeEndpoint, pane_id: Uuid, title: &str) -> EndpointEvent {
        use rttx_proto::v3_envelope::build_push_envelope;
        EndpointEvent::WorkspaceMessage {
            endpoint,
            message: build_push_envelope(v3::server_envelope::Payload::TitleChanged(
                v3::TitleChanged {
                    runtime_id: vec![],
                    pane_id: uuid_to_bytes(pane_id),
                    title: title.to_string(),
                    workspace_revision: 0,
                },
            )),
        }
    }

    fn cwd_event(endpoint: RuntimeEndpoint, pane_id: Uuid, cwd: &str) -> EndpointEvent {
        use rttx_proto::v3_envelope::build_push_envelope;
        EndpointEvent::WorkspaceMessage {
            endpoint,
            message: build_push_envelope(v3::server_envelope::Payload::CwdChanged(
                v3::CwdChanged {
                    runtime_id: vec![],
                    pane_id: uuid_to_bytes(pane_id),
                    cwd: cwd.to_string(),
                    workspace_revision: 0,
                },
            )),
        }
    }

    #[test]
    fn coalesce_merges_deltas_for_same_pane() {
        let pane = Uuid::new_v4();
        let ep = RuntimeEndpoint::Local;
        let events = vec![
            delta_event(ep.clone(), pane, b"hello ", 1),
            delta_event(ep.clone(), pane, b"world", 2),
        ];

        let batch = coalesce_event_batch(events);

        assert_eq!(batch.delta_buffers.len(), 1);
        let key = (ep, uuid_to_bytes(pane));
        assert_eq!(batch.delta_buffers[&key], b"hello world");
        assert!(batch.other_events.is_empty());
    }

    #[test]
    fn coalesce_separates_panes() {
        let pane_a = Uuid::new_v4();
        let pane_b = Uuid::new_v4();
        let ep = RuntimeEndpoint::Local;
        let events = vec![
            delta_event(ep.clone(), pane_a, b"aaa", 1),
            delta_event(ep.clone(), pane_b, b"bbb", 1),
            delta_event(ep.clone(), pane_a, b"AAA", 2),
        ];

        let batch = coalesce_event_batch(events);

        assert_eq!(batch.delta_buffers.len(), 2);
        assert_eq!(batch.delta_buffers[&(ep.clone(), uuid_to_bytes(pane_a))], b"aaaAAA");
        assert_eq!(batch.delta_buffers[&(ep, uuid_to_bytes(pane_b))], b"bbb");
        assert!(batch.other_events.is_empty());
    }

    #[test]
    fn coalesce_preserves_non_delta_events() {
        let pane = Uuid::new_v4();
        let ep = RuntimeEndpoint::Local;
        let events = vec![
            delta_event(ep.clone(), pane, b"data", 1),
            title_event(ep.clone(), pane, "my title"),
            delta_event(ep.clone(), pane, b" more", 2),
            cwd_event(ep.clone(), pane, "/tmp"),
        ];

        let batch = coalesce_event_batch(events);

        assert_eq!(batch.delta_buffers.len(), 1);
        assert_eq!(batch.delta_buffers[&(ep, uuid_to_bytes(pane))], b"data more");
        assert_eq!(batch.other_events.len(), 2);
    }

    #[test]
    fn coalesce_empty_batch() {
        let batch = coalesce_event_batch(vec![]);
        assert!(batch.delta_buffers.is_empty());
        assert!(batch.other_events.is_empty());
    }

    #[test]
    fn coalesce_no_deltas() {
        let pane = Uuid::new_v4();
        let ep = RuntimeEndpoint::Local;
        let events = vec![title_event(ep.clone(), pane, "t1"), cwd_event(ep, pane, "/home")];

        let batch = coalesce_event_batch(events);

        assert!(batch.delta_buffers.is_empty());
        assert_eq!(batch.other_events.len(), 2);
    }

    #[test]
    fn coalesce_separates_endpoints() {
        let pane = Uuid::new_v4();
        let local = RuntimeEndpoint::Local;
        let remote = RuntimeEndpoint::remote("host1");
        let events = vec![
            delta_event(local.clone(), pane, b"local", 1),
            delta_event(remote.clone(), pane, b"remote", 1),
        ];

        let batch = coalesce_event_batch(events);

        assert_eq!(batch.delta_buffers.len(), 2);
        assert_eq!(batch.delta_buffers[&(local, uuid_to_bytes(pane))], b"local");
        assert_eq!(batch.delta_buffers[&(remote, uuid_to_bytes(pane))], b"remote");
    }

    #[test]
    fn coalesce_non_delta_order_preserved() {
        let pane = Uuid::new_v4();
        let ep = RuntimeEndpoint::Local;
        let events = vec![
            cwd_event(ep.clone(), pane, "/a"),
            delta_event(ep.clone(), pane, b"x", 1),
            title_event(ep.clone(), pane, "t1"),
            cwd_event(ep.clone(), pane, "/b"),
            title_event(ep, pane, "t2"),
        ];

        let batch = coalesce_event_batch(events);

        assert_eq!(batch.other_events.len(), 4);
        // Verify order by checking the payloads
        for (i, event) in batch.other_events.iter().enumerate() {
            if let EndpointEvent::WorkspaceMessage { message, .. } = event {
                match (i, message.payload.as_ref().unwrap()) {
                    (0, v3::server_envelope::Payload::CwdChanged(c)) => {
                        assert_eq!(c.cwd, "/a");
                    }
                    (1, v3::server_envelope::Payload::TitleChanged(t)) => {
                        assert_eq!(t.title, "t1");
                    }
                    (2, v3::server_envelope::Payload::CwdChanged(c)) => {
                        assert_eq!(c.cwd, "/b");
                    }
                    (3, v3::server_envelope::Payload::TitleChanged(t)) => {
                        assert_eq!(t.title, "t2");
                    }
                    _ => panic!("unexpected event at index {i}"),
                }
            }
        }
    }
}
