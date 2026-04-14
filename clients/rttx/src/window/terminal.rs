use super::*;

impl Window {
    pub(super) fn materialize_terminal(
        &self,
        session_state: &SessionState,
        uuid: &str,
        cwd: Option<&str>,
        custom_title: Option<&str>,
    ) -> gtk4::Widget {
        if session_state.uses_managed_runtime() {
            return self.materialize_persistent_terminal(session_state, uuid, custom_title);
        }

        let zoomed = session_state.is_zoomed();
        let multi_pane = session_state.layout.terminal_count() > 1;

        let existing = {
            let terminals = self.imp().terminals.borrow();
            terminals.get(uuid).cloned()
        };
        if let Some(existing) = existing {
            if existing.parent().is_some() {
                existing.unparent();
            }
            existing.set_zoom_state(zoomed, multi_pane);
            return existing.upcast();
        }

        let term = TerminalWidget::new(uuid, cwd);
        if let Some(title) = custom_title {
            term.set_custom_title(Some(title));
        }
        term.set_zoom_state(zoomed, multi_pane);
        self.connect_terminal_signals(&term);
        self.imp().terminals.borrow_mut().insert(uuid.to_string(), term.clone());
        self.initialize_terminal_recovery(&term, session_state, uuid);
        term.upcast()
    }

    /// Create a `PersistentPaneView` for a daemon-backed session.
    fn materialize_persistent_terminal(
        &self,
        session_state: &SessionState,
        uuid: &str,
        custom_title: Option<&str>,
    ) -> gtk4::Widget {
        let zoomed = session_state.is_zoomed();
        let multi_pane = session_state.layout.terminal_count() > 1;

        let existing = {
            let panes = self.imp().persistent_terminals.borrow();
            panes.get(uuid).cloned()
        };
        if let Some(existing) = existing {
            if existing.parent().is_some() {
                existing.unparent();
            }
            existing.set_zoom_state(zoomed, multi_pane);
            return existing.upcast();
        }

        let daemon_session_id = session_state.runtime.runtime_id.as_deref().unwrap_or_default();
        let pane_view = PersistentPaneView::new(uuid, daemon_session_id);
        if let Some(title) = custom_title {
            pane_view.set_custom_title(Some(title));
        }
        pane_view.set_zoom_state(zoomed, multi_pane);
        self.apply_preferences_to_persistent_pane(&pane_view);
        self.connect_managed_pane(session_state, &pane_view);
        self.imp().persistent_terminals.borrow_mut().insert(uuid.to_string(), pane_view.clone());
        pane_view.upcast()
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

    fn connect_terminal_signals(&self, term: &TerminalWidget) {
        {
            let prefs = preferences::load();
            let font_desc = gtk4::pango::FontDescription::from_string(&prefs.font);
            let is_dark = adw::StyleManager::default().is_dark();
            let effective_name = prefs.effective_color_scheme_name(is_dark);
            let scheme = color_scheme::load_color_scheme_by_name(effective_name).or_else(|| {
                let fallback = if is_dark {
                    color_scheme::BUILTIN_DARK_SCHEME_NAME
                } else {
                    color_scheme::BUILTIN_LIGHT_SCHEME_NAME
                };
                color_scheme::load_color_scheme_by_name(fallback)
            });
            Self::apply_preferences_to_terminal(term, &prefs, &font_desc, scheme.as_ref());
        }

        let win = self.downgrade();
        let uuid = term.uuid();
        let focus_controller = gtk4::EventControllerFocus::new();
        focus_controller.connect_enter(move |_| {
            let Some(win) = win.upgrade() else { return };
            win.set_focused_terminal(Some(&uuid));
            let session_uuid = {
                let mut state = win.imp().state.borrow_mut();
                let session = state
                    .sessions
                    .iter_mut()
                    .find(|session| session.layout.contains_terminal(&uuid));
                if let Some(session) = session {
                    session.active_terminal_uuid = Some(uuid.clone());
                    Some(session.uuid.clone())
                } else {
                    None
                }
            };
            if let Some(session_uuid) = session_uuid {
                win.refresh_sidebar_subtitle(&session_uuid);
            }
        });
        term.vte().add_controller(focus_controller);

        let win = self.downgrade();
        let uuid = term.uuid();
        term.vte().connect_commit(move |_, text, _| {
            if let Some(win) = win.upgrade() {
                win.forward_input(&uuid, text);
            }
        });

        let bell_term = term.clone();
        term.vte().connect_bell(move |_| {
            bell_term.flash_bell();
        });

        let win = self.downgrade();
        let uuid = term.uuid();
        term.vte().connect_contents_changed(move |_| {
            if let Some(win) = win.upgrade() {
                win.mark_session_activity(&uuid);
            }
        });

        let win = self.downgrade();
        let uuid = term.uuid();
        term.vte().connect_window_title_changed(move |_| {
            if let Some(win) = win.upgrade() {
                win.refresh_sidebar_subtitle_if_active(&uuid);
            }
        });

        let win = self.downgrade();
        let uuid = term.uuid();
        term.vte().connect_current_directory_uri_changed(move |_| {
            if let Some(win) = win.upgrade() {
                win.refresh_sidebar_subtitle_if_active(&uuid);
            }
        });

        let drag_source = gtk4::DragSource::new();
        drag_source.set_actions(gtk4::gdk::DragAction::MOVE);
        let uuid = term.uuid();
        drag_source.connect_prepare(move |_, _, _| {
            Some(gtk4::gdk::ContentProvider::for_value(&uuid.to_value()))
        });
        term.imp().header.add_controller(drag_source);

        let drop_target = gtk4::DropTarget::new(glib::Type::STRING, gtk4::gdk::DragAction::MOVE);
        let win = self.downgrade();
        let target_uuid = term.uuid();
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
        term.add_controller(drop_target);

        let win = self.downgrade();
        let uuid = term.uuid();
        term.split_h_button().connect_clicked(move |_| {
            if let Some(win) = win.upgrade() {
                win.split_terminal(&uuid, SplitOrientation::Horizontal);
            }
        });

        let win = self.downgrade();
        let uuid = term.uuid();
        term.split_v_button().connect_clicked(move |_| {
            if let Some(win) = win.upgrade() {
                win.split_terminal(&uuid, SplitOrientation::Vertical);
            }
        });

        let win = self.downgrade();
        let uuid = term.uuid();
        term.close_button().connect_clicked(move |_| {
            if let Some(win) = win.upgrade() {
                win.close_terminal(&uuid);
            }
        });

        let win = self.downgrade();
        term.zoom_button().connect_clicked(move |_| {
            if let Some(win) = win.upgrade() {
                win.toggle_pane_zoom();
            }
        });

        let win = self.downgrade();
        let uuid = term.uuid();
        let recoverable_term = term.clone();
        let handler_id = term.vte().connect_child_exited(move |_, status| {
            recoverable_term.reset_terminal_state();
            let Some(win) = win.upgrade() else { return };
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

        let win = self.downgrade();
        let term_for_retry = term.clone();
        term.recovery_retry_button().connect_clicked(move |_| {
            if let Some(win) = win.upgrade() {
                win.retry_terminal_recovery(&term_for_retry);
            }
        });
    }

    pub(super) fn split_terminal(&self, terminal_uuid: &str, orientation: SplitOrientation) {
        // Unzoom before splitting so the full layout is visible.
        {
            let state = self.imp().state.borrow();
            if let Some(session) =
                state.sessions.iter().find(|s| s.layout.contains_terminal(terminal_uuid))
                && session.is_zoomed()
            {
                drop(state);
                self.toggle_pane_zoom();
            }
        }

        let imp = self.imp();

        let source_cwd =
            self.terminal_handle(terminal_uuid).and_then(|terminal| terminal.current_directory());

        let mut state = imp.state.borrow_mut();

        let session_idx =
            state.sessions.iter().position(|s| s.layout.contains_terminal(terminal_uuid));

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

            if let Some((mut new_layout, new_terminal_uuid)) =
                state.sessions[idx].layout.split_terminal_with_new_uuid(terminal_uuid, orientation)
            {
                // Propagate the source terminal's CWD to the new terminal node.
                if let Some(cwd) = &source_cwd {
                    new_layout.set_terminal_cwd(&new_terminal_uuid, Some(cwd.clone()));
                }
                state.sessions[idx].layout = new_layout;
                state.sessions[idx].set_recovery(&new_terminal_uuid, PaneRecovery::empty_shell());
                let layout_terminal_uuids = state.sessions[idx].layout.terminal_uuids();
                state.sessions[idx].runtime.ensure_placeholder_bindings(&layout_terminal_uuids);
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
                    self.refresh_sidebar_subtitle(&session_uuid);
                } else {
                    self.rebuild_session_content(&session_uuid, &session_state);
                }

                if session_state.uses_managed_runtime()
                    && let Some(runtime_id) = session_state.runtime.runtime_id.as_deref()
                    && let Some(manager) = self.imp().connection_manager.borrow().as_ref()
                {
                    manager.create_pane(
                        &session_uuid,
                        &session_state.runtime.endpoint,
                        runtime_id,
                        &new_terminal_uuid,
                        source_cwd,
                        adw::StyleManager::default().is_dark(),
                    );
                }
            }
        }
    }

    pub(super) fn close_terminal(&self, terminal_uuid: &str) {
        #[derive(Debug)]
        #[allow(clippy::large_enum_variant)]
        enum Action {
            CloseSession(String),
            Rebuild { session_uuid: String, session_state: SessionState },
        }

        // Unzoom before closing so the full layout is visible for removal.
        {
            let state = self.imp().state.borrow();
            if let Some(session) =
                state.sessions.iter().find(|s| s.layout.contains_terminal(terminal_uuid))
                && session.is_zoomed()
            {
                drop(state);
                self.toggle_pane_zoom();
            }
        }

        let imp = self.imp();

        let action = {
            let mut state = imp.state.borrow_mut();
            let session_idx =
                state.sessions.iter().position(|s| s.layout.contains_terminal(terminal_uuid));
            let Some(idx) = session_idx else { return };

            if state.sessions[idx].uses_managed_runtime()
                && state.sessions[idx].layout.terminal_count() > 1
                && let Some(runtime_id) = state.sessions[idx].runtime.runtime_id.clone()
                && let Some(runtime_pane_id) =
                    state.sessions[idx].runtime.pane_bindings.get(terminal_uuid).cloned()
                && runtime_pane_id != terminal_uuid
            {
                let workspace_id = state.sessions[idx].uuid.clone();
                let endpoint = state.sessions[idx].runtime.endpoint.clone();
                drop(state);
                if let Some(manager) = imp.connection_manager.borrow().as_ref() {
                    manager.close_pane(
                        &workspace_id,
                        &endpoint,
                        &runtime_id,
                        terminal_uuid,
                        &runtime_pane_id,
                    );
                }
                return;
            }

            if state.sessions[idx].layout.terminal_count() <= 1 {
                Action::CloseSession(state.sessions[idx].uuid.clone())
            } else if let Some(new_layout) =
                state.sessions[idx].layout.remove_terminal(terminal_uuid)
            {
                state.sessions[idx].layout = new_layout;
                let layout_terminal_uuids = state.sessions[idx].layout.terminal_uuids();
                state.sessions[idx].runtime.ensure_placeholder_bindings(&layout_terminal_uuids);
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

        let pre_split_position = match orientation {
            SplitOrientation::Horizontal => target.width() / 2,
            SplitOrientation::Vertical => target.height() / 2,
        };

        let inherited_cwd = target.current_directory();
        let new_term = TerminalWidget::new(new_terminal_uuid, inherited_cwd.as_deref());
        self.connect_terminal_signals(&new_term);
        imp.terminals.borrow_mut().insert(new_terminal_uuid.to_string(), new_term.clone());
        // NOTE: shell spawn is deferred until after in-place surgery succeeds,
        // to avoid leaving a live PTY if the surgery fails (#2).

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
                session::build_layout_widget(&branch_layout_clone, &|spec| {
                    if spec.uuid == target_uuid_str {
                        target_clone.clone().upcast()
                    } else if spec.uuid == new_terminal_uuid_str {
                        new_term_clone.clone().upcast()
                    } else {
                        unreachable!(
                            "split branch builder requested unexpected uuid {}",
                            spec.uuid
                        );
                    }
                })
            } else {
                gtk4::Box::new(gtk4::Orientation::Vertical, 0).upcast()
            }
        };

        if let Ok(stack) = parent.clone().downcast::<gtk4::Stack>() {
            stack.remove(&target);
            let branch = build_branch();
            if let Some(p) = branch.downcast_ref::<gtk4::Paned>() {
                let pos = pre_split_position;
                p.set_position(pos);
                p.connect_realize(move |paned| {
                    paned.set_position(pos);
                });
            }
            stack.add_named(&branch, Some(session_uuid));
            stack.set_visible_child_name(session_uuid);
            session::schedule_initial_paned_ratios(&branch, &branch_layout);
            if let Some(term) = imp.terminals.borrow().get(new_terminal_uuid) {
                term.ensure_shell_spawned_when_ready();
            }
            return true;
        }

        let Ok(paned) = parent.downcast::<gtk4::Paned>() else {
            Self::cleanup_unspliced_terminal(imp, new_terminal_uuid);
            return false;
        };

        let target_widget = target.upcast::<gtk4::Widget>();
        let start_child = paned.start_child();
        let end_child = paned.end_child();
        let is_start = start_child.as_ref() == Some(&target_widget);
        let is_end = end_child.as_ref() == Some(&target_widget);

        if !is_start && !is_end {
            Self::cleanup_unspliced_terminal(imp, new_terminal_uuid);
            return false;
        }

        if is_start {
            paned.set_start_child(None::<&gtk4::Widget>);
        } else {
            paned.set_end_child(None::<&gtk4::Widget>);
        }

        let branch = build_branch();
        if let Some(p) = branch.downcast_ref::<gtk4::Paned>() {
            let pos = pre_split_position;
            p.set_position(pos);
            p.connect_realize(move |paned| {
                paned.set_position(pos);
            });
        }
        if is_start {
            paned.set_start_child(Some(&branch));
        } else {
            paned.set_end_child(Some(&branch));
        }
        session::schedule_initial_paned_ratios(&branch, &branch_layout);
        if let Some(term) = imp.terminals.borrow().get(new_terminal_uuid) {
            term.ensure_shell_spawned_when_ready();
        }
        true
    }

    pub(super) fn rebuild_session_content(&self, session_uuid: &str, session_state: &SessionState) {
        let imp = self.imp();
        let previously_visible =
            imp.session_stack.visible_child_name().map(|name| name.to_string());

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
        let visible_after_rebuild = previously_visible
            .as_deref()
            .filter(|visible_uuid| *visible_uuid != session_uuid)
            .filter(|visible_uuid| imp.session_stack.child_by_name(visible_uuid).is_some())
            .unwrap_or(session_uuid);
        imp.session_stack.set_visible_child_name(visible_after_rebuild);
        if !session_state.is_zoomed() {
            session::schedule_initial_paned_ratios(&content, &session_state.layout);
        }

        self.remove_stale_terminal_map_entries(imp);
        self.refresh_sidebar_subtitle(session_uuid);
        self.sync_sidebar_to_visible_session();
    }

    /// Remove terminal map entries that no longer belong to any workspace layout.
    fn remove_stale_terminal_map_entries(&self, imp: &imp::Window) {
        let live_uuids: std::collections::HashSet<String> =
            imp.state.borrow().sessions.iter().flat_map(|s| s.layout.terminal_uuids()).collect();

        imp.terminals.borrow_mut().retain(|uuid, _| live_uuids.contains(uuid));
        imp.persistent_terminals.borrow_mut().retain(|uuid, _| live_uuids.contains(uuid));
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

    fn cleanup_unspliced_terminal(imp: &imp::Window, uuid: &str) {
        if let Some(term) = imp.terminals.borrow().get(uuid) {
            term.disconnect_child_exited();
        }
        imp.terminals.borrow_mut().remove(uuid);
    }

    pub(super) fn trigger_managed_recovery_for_terminal(&self, terminal_uuid: &str) {
        let Some(recovery) = self.recovery_for_terminal(terminal_uuid) else {
            return;
        };

        if let Some(target) = recovery.target.as_ref()
            && let Some(startup_input) = target.managed_startup_input()
        {
            self.send_input_to_terminal(terminal_uuid, &startup_input);
            return;
        }

        for step in recovery.startup {
            self.send_input_to_terminal(terminal_uuid, &step.terminal_input());
        }
    }

    pub(super) fn send_input_to_terminal(&self, terminal_uuid: &str, input: &str) {
        let sent = self.imp().terminals.borrow().get(terminal_uuid).cloned().map_or_else(
            || {
                if self.imp().persistent_terminals.borrow().contains_key(terminal_uuid) {
                    self.send_managed_pane_input_direct(terminal_uuid, input.as_bytes());
                    true
                } else {
                    false
                }
            },
            |term| {
                term.queue_input_for_shell(input.to_string());
                true
            },
        );

        if sent
            && let Some(terminal) = self.terminal_handle(terminal_uuid)
            && terminal.grab_focus()
        {
            self.set_focused_terminal(Some(terminal_uuid));
        }
    }

    pub(super) fn set_terminal_recovery(&self, terminal_uuid: &str, recovery: PaneRecovery) {
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
}
