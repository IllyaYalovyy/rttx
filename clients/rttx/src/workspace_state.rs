use crate::daemon_bridge::EndpointEvent;
use crate::runtime::{ConnectionStatus, RuntimeEndpoint, WorkspacePolicy, reconcile_bindings};
use crate::workspace::{LayoutNode, PaneRecovery, SplitOrientation, WindowState, WorkspaceState};
use rttx_proto::v3;
use std::collections::{BTreeMap, BTreeSet};

/// Routing metadata for a daemon-managed terminal pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedTerminalBinding {
    pub workspace_id: String,
    pub endpoint: RuntimeEndpoint,
    pub runtime_id: String,
    pub runtime_pane_id: String,
}

/// Mapping from a daemon pane snapshot to a layout terminal.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspacePaneRestore {
    pub layout_terminal_uuid: String,
    pub title: String,
    pub cwd: String,
    pub pane_output_seq: u64,
    pub scrollback_tail: bytes::Bytes,
    pub scrollback_complete: bool,
    pub cols: u16,
    pub rows: u16,
    pub terminal_modes: Option<v3::TerminalModeState>,
}

/// Pure result of reconciling a workspace against a runtime snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct ManagedWorkspaceOpenResult {
    pub session_state: WorkspaceState,
    pub panes_to_create: Vec<String>,
    pub snapshot_restores: Vec<WorkspacePaneRestore>,
    pub skipped_runtime_panes: Vec<String>,
    pub previous_layout_terminals: Vec<String>,
}

/// Pure connection-status update derived from an endpoint event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionStatusUpdate {
    pub workspace_id: String,
    pub status: ConnectionStatus,
}

/// Pure request to rebuild one workspace from mutated state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedWorkspaceRebuild {
    pub workspace_id: String,
    pub session_state: WorkspaceState,
}

/// Pure request to create a missing daemon pane for a layout terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedPaneCreateRequest {
    pub workspace_id: String,
    pub endpoint: RuntimeEndpoint,
    pub runtime_id: String,
    pub layout_terminal_uuid: String,
    pub cwd: Option<String>,
}

/// Pure outcome of reconciling a daemon endpoint event against app state.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct EndpointEventTransition {
    pub recovered_workspaces: Vec<WorkspaceState>,
    pub rebuilt_workspaces: Vec<ManagedWorkspaceRebuild>,
    pub pane_create_requests: Vec<ManagedPaneCreateRequest>,
    pub pane_snapshot_restores: Vec<WorkspacePaneRestore>,
    pub connected_layout_terminals: Vec<String>,
    pub layout_terminals_to_recover: Vec<String>,
    pub removed_layout_terminals: Vec<String>,
    pub connection_status_updates: Vec<ConnectionStatusUpdate>,
    pub skipped_runtime_panes: Vec<String>,
    pub persist_window_state: bool,
}

impl WindowState {
    /// Record a runtime ID as dismissed so inventory refresh won't resurrect it.
    pub fn dismiss_runtime(&mut self, _endpoint: &RuntimeEndpoint, runtime_id: &str) {
        self.dismissed_runtime_ids.insert(runtime_id.to_string());
    }

    /// True when startup should query an endpoint for inventory bootstrap.
    #[must_use]
    pub fn needs_inventory_bootstrap(&self, endpoint: &RuntimeEndpoint) -> bool {
        !self
            .workspaces
            .iter()
            .any(|session| session.uses_managed_runtime() && &session.runtime.endpoint == endpoint)
    }

    /// Reconcile a daemon endpoint event into pure state updates plus follow-on effects.
    #[must_use]
    pub fn reconcile_endpoint_event(&mut self, event: &EndpointEvent) -> EndpointEventTransition {
        let mut transition = EndpointEventTransition::default();

        match event {
            EndpointEvent::WorkspaceConnectionChanged { workspace_id, status } => {
                transition.connection_status_updates.push(ConnectionStatusUpdate {
                    workspace_id: workspace_id.clone(),
                    status: status.clone(),
                });
            }
            EndpointEvent::WorkspaceOpened { workspace_id, runtime_id, snapshot } => {
                let Some(opened) =
                    self.apply_managed_workspace_opened(workspace_id, runtime_id, snapshot)
                else {
                    return transition;
                };
                let ManagedWorkspaceOpenResult {
                    session_state,
                    panes_to_create,
                    snapshot_restores,
                    skipped_runtime_panes,
                    previous_layout_terminals,
                } = opened;

                let new_terminal_set: BTreeSet<_> =
                    session_state.layout.terminal_uuids().into_iter().collect();
                for old_uuid in &previous_layout_terminals {
                    if !new_terminal_set.contains(old_uuid) {
                        transition.removed_layout_terminals.push(old_uuid.clone());
                    }
                }

                transition.rebuilt_workspaces.push(ManagedWorkspaceRebuild {
                    workspace_id: workspace_id.clone(),
                    session_state: session_state.clone(),
                });
                transition.skipped_runtime_panes = skipped_runtime_panes;
                transition.pane_snapshot_restores = snapshot_restores;

                for layout_terminal_uuid in panes_to_create {
                    let cwd = session_state.layout.terminal_cwd(&layout_terminal_uuid);
                    transition.pane_create_requests.push(ManagedPaneCreateRequest {
                        workspace_id: workspace_id.clone(),
                        endpoint: session_state.runtime.endpoint.clone(),
                        runtime_id: runtime_id.clone(),
                        layout_terminal_uuid,
                        cwd,
                    });
                }
            }
            EndpointEvent::PaneCreated {
                workspace_id,
                layout_terminal_uuid,
                runtime_id,
                runtime_pane_id,
            } => {
                let applied = self.apply_managed_pane_created(
                    workspace_id,
                    layout_terminal_uuid,
                    runtime_id,
                    runtime_pane_id,
                );
                if !applied {
                    return transition;
                }

                transition.connected_layout_terminals.push(layout_terminal_uuid.clone());
                transition.layout_terminals_to_recover.push(layout_terminal_uuid.clone());
                transition.connection_status_updates.push(ConnectionStatusUpdate {
                    workspace_id: workspace_id.clone(),
                    status: ConnectionStatus::Connected,
                });
            }
            EndpointEvent::PaneClosed { workspace_id, layout_terminal_uuid, .. } => {
                let session_state =
                    self.apply_managed_pane_closed(workspace_id, layout_terminal_uuid);
                transition.removed_layout_terminals.push(layout_terminal_uuid.clone());
                if let Some(session_state) = session_state {
                    transition.rebuilt_workspaces.push(ManagedWorkspaceRebuild {
                        workspace_id: workspace_id.clone(),
                        session_state,
                    });
                }
            }
            EndpointEvent::WorkspaceDetached { workspace_id, .. } => {
                transition.connection_status_updates.push(ConnectionStatusUpdate {
                    workspace_id: workspace_id.clone(),
                    status: ConnectionStatus::Disconnected,
                });
            }
            EndpointEvent::RuntimeTerminated { workspace_id, .. } => {
                if let Some(session) =
                    self.workspaces.iter_mut().find(|session| session.uuid == *workspace_id)
                {
                    session.runtime.runtime_id = None;
                }
                transition.connection_status_updates.push(ConnectionStatusUpdate {
                    workspace_id: workspace_id.clone(),
                    status: ConnectionStatus::Disconnected,
                });
            }
            EndpointEvent::InventoryLoaded { endpoint, runtimes } => {
                let inventory_ids: std::collections::BTreeSet<String> = runtimes
                    .iter()
                    .filter_map(|s| rttx_proto::bytes_to_uuid(&s.id).ok().map(|u| u.to_string()))
                    .collect();
                self.dismissed_runtime_ids.retain(|id| inventory_ids.contains(id));

                let recovered = self.recover_managed_workspaces_from_inventory(endpoint, runtimes);
                if recovered.is_empty() {
                    return transition;
                }

                for session_state in &recovered {
                    transition.recovered_workspaces.push(session_state.clone());
                    transition.connection_status_updates.push(ConnectionStatusUpdate {
                        workspace_id: session_state.uuid.clone(),
                        status: ConnectionStatus::Connecting,
                    });
                }
                transition.persist_window_state = true;
            }
            EndpointEvent::WorkspaceResynced { workspace_id, snapshot, .. } => {
                let restores = self.build_resync_restores(workspace_id, snapshot);
                transition.pane_snapshot_restores = restores;
            }
            EndpointEvent::RuntimeMessage { .. } | EndpointEvent::WorkspaceError { .. } => {}
        }

        transition
    }

    /// Recover daemon-managed workspaces that exist in inventory but not in the GUI state.
    pub fn recover_managed_workspaces_from_inventory(
        &mut self,
        endpoint: &RuntimeEndpoint,
        runtimes: &[v3::RuntimeInfo],
    ) -> Vec<WorkspaceState> {
        let mut known_runtime_ids = self
            .workspaces
            .iter()
            .filter(|session| {
                session.uses_managed_runtime() && &session.runtime.endpoint == endpoint
            })
            .filter_map(|session| session.runtime.runtime_id.clone())
            .collect::<BTreeSet<_>>();
        let mut recovered = Vec::new();

        for rt_info in runtimes {
            let Some(session) = recovered_managed_workspace(endpoint, rt_info) else {
                continue;
            };
            let Some(runtime_id) = session.runtime.runtime_id.clone() else {
                continue;
            };
            if self.dismissed_runtime_ids.contains(&runtime_id) {
                continue;
            }
            if !known_runtime_ids.insert(runtime_id) {
                continue;
            }
            self.workspaces.push(session.clone());
            recovered.push(session);
        }

        if !recovered.is_empty() {
            self.rebuild_pane_reverse_index();
        }

        recovered
    }

    /// Resolve a layout terminal to its managed runtime binding.
    #[must_use]
    pub fn managed_terminal_binding(&self, terminal_uuid: &str) -> Option<ManagedTerminalBinding> {
        let session = self.workspaces.iter().find(|session| {
            session.uses_managed_runtime() && session.layout.contains_terminal(terminal_uuid)
        })?;
        let runtime_id = session.runtime.runtime_id.clone()?;
        let runtime_pane_id = session.runtime.pane_bindings.get(terminal_uuid)?.clone();
        if session.runtime.is_layout_pane_pending(terminal_uuid) {
            return None;
        }
        Some(ManagedTerminalBinding {
            workspace_id: session.uuid.clone(),
            endpoint: session.runtime.endpoint.clone(),
            runtime_id,
            runtime_pane_id,
        })
    }

    /// Return managed bindings for all sibling panes when input sync is active.
    ///
    /// Returns an empty vec when input sync is off, the workspace is not
    /// managed, or the source terminal has no routable binding.
    #[must_use]
    pub fn input_sync_targets(&self, source_uuid: &str) -> Vec<ManagedTerminalBinding> {
        let session = self.workspaces.iter().find(|s| {
            s.input_sync && s.uses_managed_runtime() && s.layout.contains_terminal(source_uuid)
        });
        let Some(session) = session else {
            return Vec::new();
        };
        let Some(runtime_id) = session.runtime.runtime_id.as_deref() else {
            return Vec::new();
        };
        session
            .layout
            .terminal_uuids()
            .into_iter()
            .filter(|uuid| uuid != source_uuid)
            .filter_map(|uuid| {
                let pane_id = session.runtime.pane_bindings.get(&uuid)?;
                if session.runtime.is_layout_pane_pending(&uuid) {
                    return None;
                }
                Some(ManagedTerminalBinding {
                    workspace_id: session.uuid.clone(),
                    endpoint: session.runtime.endpoint.clone(),
                    runtime_id: runtime_id.to_string(),
                    runtime_pane_id: pane_id.clone(),
                })
            })
            .collect()
    }

    /// Resolve a runtime ID back to its workspace.
    #[must_use]
    pub fn workspace_for_runtime(
        &self,
        endpoint: &RuntimeEndpoint,
        runtime_id: &str,
    ) -> Option<String> {
        self.workspaces
            .iter()
            .find(|session| {
                session.uses_managed_runtime()
                    && &session.runtime.endpoint == endpoint
                    && session.runtime.runtime_id.as_deref() == Some(runtime_id)
            })
            .map(|session| session.uuid.clone())
    }

    /// Resolve a runtime pane back to its workspace/layout terminal.
    #[must_use]
    pub fn runtime_pane_target(
        &self,
        endpoint: &RuntimeEndpoint,
        runtime_pane_id: &str,
    ) -> Option<(String, String)> {
        let key = Self::pane_index_key(&endpoint.key(), runtime_pane_id);
        self.pane_reverse_index.get(&key).cloned()
    }

    /// Apply the state mutation for a daemon pane-create acknowledgement.
    pub fn apply_managed_pane_created(
        &mut self,
        workspace_id: &str,
        layout_terminal_uuid: &str,
        runtime_id: &str,
        runtime_pane_id: &str,
    ) -> bool {
        let Some(session) = self.workspaces.iter_mut().find(|session| session.uuid == workspace_id)
        else {
            return false;
        };
        if !session.layout.contains_terminal(layout_terminal_uuid) {
            return false;
        }

        session.runtime.runtime_id = Some(runtime_id.to_string());
        session.runtime.bind_runtime_pane(layout_terminal_uuid, runtime_pane_id);
        self.rebuild_pane_reverse_index();
        true
    }

    /// Apply the state mutation for a daemon-acked managed pane close.
    pub fn apply_managed_pane_closed(
        &mut self,
        workspace_id: &str,
        layout_terminal_uuid: &str,
    ) -> Option<WorkspaceState> {
        let session = self.workspaces.iter_mut().find(|session| session.uuid == workspace_id)?;
        session.runtime.pane_bindings.remove(layout_terminal_uuid);
        let new_layout = session.layout.remove_terminal(layout_terminal_uuid)?;
        session.layout = new_layout;
        let layout_terminal_uuids = session.layout.terminal_uuids();
        session.runtime.ensure_placeholder_bindings(&layout_terminal_uuids);
        session.prune_recovery();
        session.normalize_active_terminal();
        let result = session.clone();
        self.rebuild_pane_reverse_index();
        Some(result)
    }

    /// Apply the state mutation for attaching/reconciling a managed runtime snapshot.
    pub fn apply_managed_workspace_opened(
        &mut self,
        workspace_id: &str,
        runtime_id: &str,
        snapshot: &v3::RuntimeSnapshot,
    ) -> Option<ManagedWorkspaceOpenResult> {
        let session = self.workspaces.iter_mut().find(|session| session.uuid == workspace_id)?;

        let had_runtime_id = session.runtime.runtime_id.is_some();
        session.runtime.runtime_id = Some(runtime_id.to_string());

        let layout_terminal_uuids = session.layout.terminal_uuids();
        let runtime_pane_uuids =
            snapshot.panes.iter().filter_map(snapshot_pane_id).collect::<Vec<_>>();
        let mut reconciliation = reconcile_bindings(
            &layout_terminal_uuids,
            &session.runtime.pane_bindings,
            &runtime_pane_uuids,
        );

        // Match disconnected layout terminals to unclaimed runtime panes by
        // position. This covers both placeholder bindings (state saved before
        // PaneCreated arrived) and stale bindings (daemon restarted with new
        // pane IDs), preventing layout growth on repeated reconnect cycles.
        if !reconciliation.disconnected_layout_panes.is_empty()
            && !reconciliation.recovered_runtime_panes.is_empty()
        {
            let mut recovered =
                reconciliation.recovered_runtime_panes.drain(..).collect::<Vec<_>>();
            let mut still_disconnected = Vec::new();
            for layout_uuid in reconciliation.disconnected_layout_panes.drain(..) {
                if let Some(runtime_pane_id) = recovered.first().cloned() {
                    recovered.remove(0);
                    reconciliation.bindings.insert(layout_uuid, runtime_pane_id);
                } else {
                    still_disconnected.push(layout_uuid);
                }
            }
            reconciliation.disconnected_layout_panes = still_disconnected;
            reconciliation.recovered_runtime_panes = recovered;
        }

        session.runtime.pane_bindings = reconciliation.bindings;
        session.runtime.pending_layout_panes =
            reconciliation.disconnected_layout_panes.iter().cloned().collect();

        let panes_to_create = if had_runtime_id {
            // Reconnecting: create panes for any layout terminals that
            // couldn't be matched to existing runtime panes (covers both
            // placeholder-only and stale-binding reconnects).
            reconciliation.disconnected_layout_panes.clone()
        } else {
            let mut placeholders = reconciliation.disconnected_layout_panes.clone();
            if let Some(initial_terminal_uuid) = layout_terminal_uuids.first() {
                placeholders
                    .retain(|layout_terminal_uuid| layout_terminal_uuid != initial_terminal_uuid);
            }
            placeholders
        };

        let mut skipped_runtime_panes = Vec::new();
        for runtime_pane_id in reconciliation.recovered_runtime_panes {
            let Some(anchor_uuid) = session.layout.terminal_uuids().last().cloned() else {
                skipped_runtime_panes.push(runtime_pane_id);
                continue;
            };
            let anchor_cwd = session.layout.terminal_cwd(&anchor_uuid);
            let Some((mut new_layout, new_terminal_uuid)) = session
                .layout
                .split_terminal_with_new_uuid(&anchor_uuid, SplitOrientation::Horizontal)
            else {
                skipped_runtime_panes.push(runtime_pane_id);
                continue;
            };

            if let Some(cwd) = anchor_cwd {
                new_layout.set_terminal_cwd(&new_terminal_uuid, Some(cwd));
            }
            session.layout = new_layout;
            session.set_recovery(&new_terminal_uuid, PaneRecovery::empty_shell());
            session.runtime.bind_runtime_pane(&new_terminal_uuid, &runtime_pane_id);
            session.normalize_active_terminal();
        }

        let layout_by_runtime_pane = session
            .runtime
            .pane_bindings
            .iter()
            .map(|(layout_terminal_uuid, runtime_pane_id)| {
                (runtime_pane_id.clone(), layout_terminal_uuid.clone())
            })
            .collect::<BTreeMap<_, _>>();
        let snapshot_restores = snapshot
            .panes
            .iter()
            .filter_map(|pane_snapshot| {
                let runtime_pane_id = snapshot_pane_id(pane_snapshot)?;
                let layout_terminal_uuid = layout_by_runtime_pane.get(&runtime_pane_id)?.clone();
                Some(WorkspacePaneRestore {
                    layout_terminal_uuid,
                    title: pane_snapshot.title.clone(),
                    cwd: pane_snapshot.cwd.clone(),
                    pane_output_seq: pane_snapshot.pane_output_seq,
                    scrollback_tail: pane_snapshot.scrollback_tail.clone(),
                    scrollback_complete: pane_snapshot.scrollback_complete,
                    cols: pane_snapshot.cols as u16,
                    rows: pane_snapshot.rows as u16,
                    terminal_modes: pane_snapshot.terminal_modes,
                })
            })
            .collect::<Vec<_>>();

        // Update layout CWDs from the snapshot so the rebuilt session
        // carries the daemon's current CWD, not stale client-side values.
        for restore in &snapshot_restores {
            if !restore.cwd.is_empty() {
                session
                    .layout
                    .set_terminal_cwd(&restore.layout_terminal_uuid, Some(restore.cwd.clone()));
            }
        }

        let session_state = session.clone();
        self.rebuild_pane_reverse_index();

        Some(ManagedWorkspaceOpenResult {
            session_state,
            panes_to_create,
            snapshot_restores,
            skipped_runtime_panes,
            previous_layout_terminals: layout_terminal_uuids,
        })
    }

    /// Build snapshot restores for a resync without rebuilding the layout.
    fn build_resync_restores(
        &self,
        workspace_id: &str,
        snapshot: &v3::RuntimeSnapshot,
    ) -> Vec<WorkspacePaneRestore> {
        let Some(session) = self.workspaces.iter().find(|s| s.uuid == workspace_id) else {
            return Vec::new();
        };
        let layout_by_runtime_pane: BTreeMap<_, _> = session
            .runtime
            .pane_bindings
            .iter()
            .map(|(layout_uuid, runtime_pane_id)| (runtime_pane_id.clone(), layout_uuid.clone()))
            .collect();
        snapshot
            .panes
            .iter()
            .filter_map(|pane_snapshot| {
                let runtime_pane_id = snapshot_pane_id(pane_snapshot)?;
                let layout_terminal_uuid = layout_by_runtime_pane.get(&runtime_pane_id)?.clone();
                Some(WorkspacePaneRestore {
                    layout_terminal_uuid,
                    title: pane_snapshot.title.clone(),
                    cwd: pane_snapshot.cwd.clone(),
                    pane_output_seq: pane_snapshot.pane_output_seq,
                    scrollback_tail: pane_snapshot.scrollback_tail.clone(),
                    scrollback_complete: pane_snapshot.scrollback_complete,
                    cols: pane_snapshot.cols as u16,
                    rows: pane_snapshot.rows as u16,
                    terminal_modes: pane_snapshot.terminal_modes,
                })
            })
            .collect()
    }
}

fn recovered_managed_workspace(
    endpoint: &RuntimeEndpoint,
    rt_info: &v3::RuntimeInfo,
) -> Option<WorkspaceState> {
    let runtime_id = rttx_proto::bytes_to_uuid(&rt_info.id).ok()?.to_string();
    let policy = match v3::RuntimePolicy::try_from(rt_info.policy).ok() {
        Some(v3::RuntimePolicy::Ephemeral) => WorkspacePolicy::Ephemeral,
        _ => WorkspacePolicy::Persistent,
    };

    let mut session = WorkspaceState::new_managed_local(rt_info.name.clone(), policy, None);
    session.uuid = inventory_workspace_id(endpoint, &runtime_id);
    session.runtime.endpoint = endpoint.clone();
    session.runtime.runtime_id = Some(runtime_id);

    if !rt_info.panes.is_empty() {
        session.layout = layout_from_inventory_panes(&rt_info.panes);
        let pane_ids = session.layout.terminal_uuids();
        session.terminal_recovery = pane_ids
            .iter()
            .cloned()
            .map(|pane_id| (pane_id, PaneRecovery::empty_shell()))
            .collect();
        session.active_terminal_uuid = pane_ids.first().cloned();
        session.runtime.pane_bindings =
            pane_ids.iter().cloned().map(|pane_id| (pane_id.clone(), pane_id)).collect();
        session.runtime.pending_layout_panes.clear();
    }

    Some(session)
}

fn inventory_workspace_id(endpoint: &RuntimeEndpoint, runtime_id: &str) -> String {
    format!("inventory:{}:{runtime_id}", endpoint.key())
}

fn layout_from_inventory_panes(panes: &[v3::PaneInfo]) -> LayoutNode {
    let mut panes = panes.iter();
    let Some(first_pane) = panes.next() else {
        return LayoutNode::new_terminal();
    };

    panes.fold(layout_terminal_from_inventory(first_pane), |layout, pane| LayoutNode::Split {
        orientation: SplitOrientation::Horizontal,
        ratio: 0.5,
        first: Box::new(layout),
        second: Box::new(layout_terminal_from_inventory(pane)),
    })
}

fn layout_terminal_from_inventory(pane: &v3::PaneInfo) -> LayoutNode {
    LayoutNode::Terminal {
        uuid: inventory_pane_id(pane).unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        profile: None,
        cwd: (!pane.cwd.is_empty()).then(|| pane.cwd.clone()),
        custom_title: (!pane.title.is_empty()).then(|| pane.title.clone()),
    }
}

fn inventory_pane_id(pane: &v3::PaneInfo) -> Option<String> {
    rttx_proto::bytes_to_uuid(&pane.id).ok().map(|uuid| uuid.to_string())
}

fn snapshot_pane_id(pane_snapshot: &v3::PaneSnapshot) -> Option<String> {
    rttx_proto::bytes_to_uuid(&pane_snapshot.pane_id).ok().map(|uuid| uuid.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_bridge::EndpointEvent;
    use crate::runtime::ConnectionStatus;
    use crate::runtime::WorkspacePolicy;
    use crate::test_helpers::{
        hsplit, managed_session, managed_session_with_runtime, term, term_full, window_state,
        workspace,
    };

    fn pane_snapshot(pane_id: &str, title: &str, cwd: &str, scrollback: &[u8]) -> v3::PaneSnapshot {
        v3::PaneSnapshot {
            pane_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(pane_id).unwrap()),
            pane_output_seq: 0,
            title: title.to_string(),
            cwd: cwd.to_string(),
            cols: 120,
            rows: 40,
            exit_status: None,
            terminal_modes: None,
            scrollback_tail: bytes::Bytes::copy_from_slice(scrollback),
            total_scrollback_bytes: scrollback.len() as u64,
            scrollback_complete: true,
        }
    }

    fn snapshot(runtime_id: &str, panes: Vec<v3::PaneSnapshot>) -> v3::RuntimeSnapshot {
        v3::RuntimeSnapshot {
            tree: None,
            default_active_pane_id: Vec::new(),
            runtime_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(runtime_id).unwrap()),
            panes,
            runtime_revision: 7,
            client_role: v3::RuntimeClientRole::Writer as i32,
        }
    }

    fn pane_info(pane_id: &str, title: &str, cwd: &str) -> v3::PaneInfo {
        v3::PaneInfo {
            id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(pane_id).unwrap()),
            title: title.to_string(),
            cwd: cwd.to_string(),
            cols: 120,
            rows: 40,
            exit_status: None,
            reconstructed: true,
            no_persist: false,
        }
    }

    fn rt_info(
        runtime_id: &str,
        name: &str,
        policy: v3::RuntimePolicy,
        panes: Vec<v3::PaneInfo>,
        _active_pane_id: Option<&str>,
    ) -> v3::RuntimeInfo {
        v3::RuntimeInfo {
            id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(runtime_id).unwrap()),
            name: name.to_string(),
            pane_count: panes.len() as u32,
            panes,
            policy: policy as i32,
            reconstructed: true,
            runtime_revision: 7,
            current_client_role: v3::RuntimeClientRole::Unattached as i32,
            has_write_owner: false,
            read_only_client_count: 0,
            active_pane_summary: String::new(),
            takeover_eligible: false,
            disabled_reason: String::new(),
        }
    }

    /// `WorkspaceMode` was removed — serialized state must not contain it.
    #[test]
    fn serialized_workspace_state_has_no_mode_field() {
        let state = window_state(vec![managed_session("ws-1", "Workspace", term("pane-1"))]);
        let json = serde_json::to_string(&state.workspaces[0]).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value.get("mode").is_none(), "WorkspaceMode must be fully removed");
    }

    #[test]
    fn managed_terminal_binding_ignores_placeholders_and_uses_explicit_runtime_bindings() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let runtime_pane_id = uuid::Uuid::new_v4().to_string();
        let mut session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            term("pane-1"),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(&runtime_id),
        );
        let mut state = window_state(vec![session.clone()]);

        assert!(
            state.managed_terminal_binding("pane-1").is_none(),
            "self-bindings stay unroutable until the daemon assigns a pane id",
        );

        session.runtime.bind_runtime_pane("pane-1", &runtime_pane_id);
        state.workspaces[0] = session;

        let binding = state
            .managed_terminal_binding("pane-1")
            .expect("explicit daemon binding should resolve");
        assert_eq!(binding.workspace_id, "workspace-1");
        assert_eq!(binding.endpoint, RuntimeEndpoint::Local);
        assert_eq!(binding.runtime_id, runtime_id);
        assert_eq!(binding.runtime_pane_id, runtime_pane_id);
    }

    #[test]
    fn runtime_lookup_helpers_resolve_workspace_and_pane_targets() {
        let endpoint = RuntimeEndpoint::remote("builder.example");
        let mut session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            term("pane-1"),
            endpoint.clone(),
            WorkspacePolicy::Persistent,
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
        session.runtime.bind_runtime_pane("pane-1", "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26");
        let state = window_state(vec![session]);

        assert_eq!(
            state.workspace_for_runtime(&endpoint, "d7d04564-b2bf-4302-9495-e65c4df12ac6"),
            Some("workspace-1".into()),
        );
        assert_eq!(
            state.runtime_pane_target(&endpoint, "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26"),
            Some(("workspace-1".into(), "pane-1".into())),
        );
    }

    #[test]
    fn apply_managed_pane_created_binds_runtime_and_clears_pending_placeholder() {
        let mut state =
            window_state(vec![managed_session("workspace-1", "Workspace", term("pane-1"))]);

        assert!(state.apply_managed_pane_created(
            "workspace-1",
            "pane-1",
            "d7d04564-b2bf-4302-9495-e65c4df12ac6",
            "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26",
        ));

        let session = &state.workspaces[0];
        assert_eq!(
            session.runtime.runtime_id.as_deref(),
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
        assert_eq!(
            session.runtime.pane_bindings.get("pane-1").map(String::as_str),
            Some("598b80fe-b96b-4fbf-8e2d-f2610b6f4f26"),
        );
        assert!(!session.runtime.is_layout_pane_pending("pane-1"));
        assert_eq!(
            session.runtime.runtime_id.as_deref(),
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
    }

    #[test]
    fn apply_managed_pane_created_rejects_unknown_layout_terminal() {
        let mut state =
            window_state(vec![managed_session("workspace-1", "Workspace", term("pane-1"))]);
        let before = state.clone();

        assert!(!state.apply_managed_pane_created(
            "workspace-1",
            "missing-pane",
            "d7d04564-b2bf-4302-9495-e65c4df12ac6",
            "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26",
        ));
        assert_eq!(state, before);
    }

    #[test]
    fn apply_managed_pane_closed_prunes_state_and_preserves_remaining_binding() {
        let mut session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            hsplit(term("left"), term("right")),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
        session.runtime.bind_runtime_pane("left", "07fa83b4-9ae3-4354-a1c5-1f685ffab370");
        session.runtime.bind_runtime_pane("right", "0d88f17f-626d-40b8-a1d3-6a42af628ac9");
        session.active_terminal_uuid = Some("left".into());
        let mut state = window_state(vec![session]);

        let updated = state
            .apply_managed_pane_closed("workspace-1", "left")
            .expect("removing one branch of a managed split should preserve the workspace");

        assert_eq!(updated.layout.terminal_uuids(), vec!["right".to_string()]);
        assert_eq!(updated.active_terminal_uuid.as_deref(), Some("right"));
        assert_eq!(
            updated.runtime.pane_bindings.get("right").map(String::as_str),
            Some("0d88f17f-626d-40b8-a1d3-6a42af628ac9"),
        );
        assert!(updated.recovery_for("left").is_none());
        assert!(updated.recovery_for("right").is_some());
    }

    #[test]
    fn apply_managed_workspace_opened_recovers_extra_runtime_panes_in_pure_state() {
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let first_runtime_pane = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
        let second_runtime_pane = "0d88f17f-626d-40b8-a1d3-6a42af628ac9";
        let mut session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            term_full("left", "/srv/project", "Left"),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(runtime_id),
        );
        session.runtime.bind_runtime_pane("left", first_runtime_pane);
        let mut state = window_state(vec![session]);
        let snapshot = snapshot(
            runtime_id,
            vec![
                pane_snapshot(first_runtime_pane, "Shell", "/srv/project", b"shell"),
                pane_snapshot(second_runtime_pane, "Logs", "/srv/project", b"logs"),
            ],
        );

        let opened = state
            .apply_managed_workspace_opened("workspace-1", runtime_id, &snapshot)
            .expect("managed workspace open should reconcile state");

        assert!(opened.panes_to_create.is_empty());
        assert!(opened.skipped_runtime_panes.is_empty());
        assert_eq!(opened.snapshot_restores.len(), 2);
        assert_eq!(opened.session_state.layout.terminal_count(), 2);

        let recovered_terminal_uuid = opened
            .session_state
            .layout
            .terminal_uuids()
            .into_iter()
            .find(|uuid| uuid != "left")
            .expect("extra runtime pane should synthesize a recovered layout pane");
        assert_eq!(
            opened
                .session_state
                .runtime
                .pane_bindings
                .get(&recovered_terminal_uuid)
                .map(String::as_str),
            Some(second_runtime_pane),
        );
        assert_eq!(
            opened.session_state.layout.terminal_cwd(&recovered_terminal_uuid).as_deref(),
            Some("/srv/project"),
            "recovered pane should inherit the anchor cwd in pure state",
        );
        assert_eq!(
            opened
                .session_state
                .recovery_for(&recovered_terminal_uuid)
                .expect("recovered pane should have default recovery"),
            &PaneRecovery::empty_shell(),
        );
        assert!(opened.snapshot_restores.iter().any(|restore| {
            restore.layout_terminal_uuid == "left"
                && restore.title == "Shell"
                && restore.cwd == "/srv/project"
                && restore.scrollback_tail[..] == b"shell"[..]
        }));
        assert!(opened.snapshot_restores.iter().any(|restore| {
            restore.layout_terminal_uuid == recovered_terminal_uuid
                && restore.title == "Logs"
                && restore.cwd == "/srv/project"
                && restore.scrollback_tail[..] == b"logs"[..]
        }));
    }

    #[test]
    fn recover_managed_workspaces_from_inventory_creates_recoverable_layouts() {
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let first_pane = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
        let second_pane = "0d88f17f-626d-40b8-a1d3-6a42af628ac9";
        let mut state = window_state(vec![]);

        let recovered = state.recover_managed_workspaces_from_inventory(
            &RuntimeEndpoint::Local,
            &[rt_info(
                runtime_id,
                "Recovered Workspace",
                v3::RuntimePolicy::Persistent,
                vec![
                    pane_info(first_pane, "Shell", "/srv/project"),
                    pane_info(second_pane, "Logs", "/srv/project"),
                ],
                Some(second_pane),
            )],
        );

        assert_eq!(recovered.len(), 1);
        assert_eq!(state.workspaces.len(), 1);
        let session = &recovered[0];
        assert_eq!(session.uuid, "inventory:local:d7d04564-b2bf-4302-9495-e65c4df12ac6");
        assert_eq!(session.name, "Recovered Workspace");
        assert_eq!(session.runtime.endpoint, RuntimeEndpoint::Local);
        assert_eq!(session.runtime.policy, WorkspacePolicy::Persistent);
        assert_eq!(session.runtime.runtime_id.as_deref(), Some(runtime_id));
        assert_eq!(session.active_terminal_uuid.as_deref(), Some(first_pane));
        assert_eq!(
            session.layout.terminal_uuids(),
            vec![first_pane.to_string(), second_pane.to_string()]
        );
        assert_eq!(
            session.runtime.pane_bindings.get(first_pane).map(String::as_str),
            Some(first_pane)
        );
        assert_eq!(
            session.runtime.pane_bindings.get(second_pane).map(String::as_str),
            Some(second_pane)
        );
        assert!(session.runtime.pending_layout_panes.is_empty());

        let snapshot = snapshot(
            runtime_id,
            vec![
                pane_snapshot(first_pane, "Shell", "/srv/project", b"shell"),
                pane_snapshot(second_pane, "Logs", "/srv/project", b"logs"),
            ],
        );
        let opened = state
            .apply_managed_workspace_opened(&session.uuid, runtime_id, &snapshot)
            .expect("inventory-recovered workspace should reattach cleanly");
        assert!(opened.panes_to_create.is_empty());
        assert!(opened.skipped_runtime_panes.is_empty());
        assert_eq!(opened.session_state.layout.terminal_uuids(), session.layout.terminal_uuids());
        assert_eq!(opened.snapshot_restores.len(), 2);
    }

    #[test]
    fn recover_managed_workspaces_from_inventory_skips_known_runtime_ids() {
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let existing = managed_session_with_runtime(
            "workspace-1",
            "Existing Workspace",
            term("pane-1"),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(runtime_id),
        );
        let mut state = window_state(vec![existing]);

        let recovered = state.recover_managed_workspaces_from_inventory(
            &RuntimeEndpoint::Local,
            &[rt_info(
                runtime_id,
                "Recovered Workspace",
                v3::RuntimePolicy::Persistent,
                vec![pane_info("07fa83b4-9ae3-4354-a1c5-1f685ffab370", "Shell", "/srv/project")],
                None,
            )],
        );

        assert!(recovered.is_empty());
        assert_eq!(state.workspaces.len(), 1);
        assert_eq!(state.workspaces[0].uuid, "workspace-1");
    }

    #[test]
    fn apply_managed_workspace_opened_creates_initial_pane_for_empty_inventory_runtime() {
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let mut state = window_state(vec![]);

        let recovered = state.recover_managed_workspaces_from_inventory(
            &RuntimeEndpoint::Local,
            &[rt_info(
                runtime_id,
                "Recovered Workspace",
                v3::RuntimePolicy::Persistent,
                vec![],
                None,
            )],
        );

        let session =
            recovered.first().expect("inventory should synthesize a placeholder workspace");
        let placeholder_uuid = session.layout.terminal_uuids()[0].clone();
        let opened = state
            .apply_managed_workspace_opened(
                &session.uuid,
                runtime_id,
                &snapshot(runtime_id, vec![]),
            )
            .expect("empty runtime should still reconcile");

        assert_eq!(opened.panes_to_create, vec![placeholder_uuid]);
        assert!(opened.snapshot_restores.is_empty());
    }

    #[test]
    fn reconcile_endpoint_event_status_change_emits_workspace_status_update() {
        let mut state = WindowState::default_for_test();

        let transition =
            state.reconcile_endpoint_event(&EndpointEvent::WorkspaceConnectionChanged {
                workspace_id: "workspace-1".into(),
                status: ConnectionStatus::Reconnecting { attempt: 2, retry_in_secs: 4 },
            });

        assert_eq!(
            transition.connection_status_updates,
            vec![ConnectionStatusUpdate {
                workspace_id: "workspace-1".into(),
                status: ConnectionStatus::Reconnecting { attempt: 2, retry_in_secs: 4 },
            }],
        );
    }

    #[test]
    fn reconcile_endpoint_event_inventory_loaded_recovers_workspace_and_persists() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let pane_id = uuid::Uuid::new_v4().to_string();
        let mut state = WindowState::default_for_test();

        let transition = state.reconcile_endpoint_event(&EndpointEvent::InventoryLoaded {
            endpoint: RuntimeEndpoint::Local,
            runtimes: vec![rt_info(
                &runtime_id,
                "Recovered Workspace",
                v3::RuntimePolicy::Persistent,
                vec![pane_info(&pane_id, "Shell", "/srv/project")],
                Some(&pane_id),
            )],
        });

        assert_eq!(transition.recovered_workspaces.len(), 1);
        assert!(transition.persist_window_state);
        assert_eq!(
            transition.connection_status_updates,
            vec![ConnectionStatusUpdate {
                workspace_id: format!("inventory:local:{runtime_id}"),
                status: ConnectionStatus::Connecting,
            }],
        );
        assert!(
            state
                .workspaces
                .iter()
                .any(|session| session.uuid == format!("inventory:local:{runtime_id}"))
        );
    }

    #[test]
    fn needs_inventory_bootstrap_when_no_managed_workspace_uses_endpoint() {
        let state = window_state(vec![workspace("session-1", "Session 1", term("pane-1"))]);

        assert!(state.needs_inventory_bootstrap(&RuntimeEndpoint::Local));
        assert!(state.needs_inventory_bootstrap(&RuntimeEndpoint::remote("builder.example")));
    }

    #[test]
    fn needs_inventory_bootstrap_skips_endpoints_already_present_in_state() {
        let state = window_state(vec![
            managed_session_with_runtime(
                "workspace-1",
                "Local Workspace",
                term("local-pane"),
                RuntimeEndpoint::Local,
                WorkspacePolicy::Persistent,
                Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
            ),
            managed_session_with_runtime(
                "workspace-2",
                "Remote Workspace",
                term("remote-pane"),
                RuntimeEndpoint::remote("builder.example"),
                WorkspacePolicy::Persistent,
                Some("598b80fe-b96b-4fbf-8e2d-f2610b6f4f26"),
            ),
        ]);

        assert!(!state.needs_inventory_bootstrap(&RuntimeEndpoint::Local));
        assert!(!state.needs_inventory_bootstrap(&RuntimeEndpoint::remote("builder.example")));
        assert!(state.needs_inventory_bootstrap(&RuntimeEndpoint::remote("other.example")));
    }

    #[test]
    fn reconcile_endpoint_event_workspace_opened_rebuilds_restores_and_requests_missing_panes() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let first_terminal_uuid = uuid::Uuid::new_v4().to_string();
        let second_terminal_uuid = uuid::Uuid::new_v4().to_string();
        let mut state = window_state(vec![managed_session(
            "workspace-1",
            "Workspace",
            hsplit(term(&first_terminal_uuid), term(&second_terminal_uuid)),
        )]);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "workspace-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(
                &runtime_id,
                vec![pane_snapshot(
                    &first_terminal_uuid,
                    "Shell",
                    "/srv/project",
                    b"restored output",
                )],
            ),
        });

        assert_eq!(
            transition.rebuilt_workspaces,
            vec![ManagedWorkspaceRebuild {
                workspace_id: "workspace-1".into(),
                session_state: state.workspaces[0].clone(),
            }],
        );
        assert_eq!(
            transition.pane_create_requests,
            vec![ManagedPaneCreateRequest {
                workspace_id: "workspace-1".into(),
                endpoint: RuntimeEndpoint::Local,
                runtime_id: runtime_id.clone(),
                layout_terminal_uuid: second_terminal_uuid,
                cwd: None,
            }],
        );
        assert_eq!(
            transition.pane_snapshot_restores,
            vec![WorkspacePaneRestore {
                layout_terminal_uuid: first_terminal_uuid,
                title: "Shell".into(),
                cwd: "/srv/project".into(),
                pane_output_seq: 0,
                scrollback_tail: bytes::Bytes::from_static(b"restored output"),
                scrollback_complete: true,
                cols: 120,
                rows: 40,
                terminal_modes: None,
            }],
        );
        assert_eq!(state.workspaces[0].runtime.runtime_id.as_deref(), Some(runtime_id.as_str()));
    }

    #[test]
    fn reconcile_snapshot_carries_interaction_modes_to_restore() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let terminal_uuid = uuid::Uuid::new_v4().to_string();
        let mut state =
            window_state(vec![managed_session("workspace-1", "Workspace", term(&terminal_uuid))]);

        let mut snap = pane_snapshot(&terminal_uuid, "vim", "/home", b"");
        snap.terminal_modes = Some(v3::TerminalModeState {
            bracketed_paste: true,
            focus_reporting: false,
            application_cursor_keys: true,
            application_keypad: true,
            alternate_screen: false,
            cursor_hidden: false,
            mouse_mode: v3::MouseMode::Any as i32,
            sgr_mouse: true,
        });

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "workspace-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(&runtime_id, vec![snap]),
        });

        assert_eq!(transition.pane_snapshot_restores.len(), 1);
        let restore = &transition.pane_snapshot_restores[0];
        let modes = restore.terminal_modes.as_ref().expect("terminal_modes should be present");
        assert!(modes.application_cursor_keys);
        assert!(modes.application_keypad);
        assert_eq!(modes.mouse_mode, v3::MouseMode::Any as i32);
        assert!(modes.sgr_mouse);
        assert!(modes.bracketed_paste);
    }

    /// Snapshot with `focus_reporting` and `cursor_hidden` propagates through
    /// reconciliation so the restore path can re-apply them. #765.
    #[test]
    fn reconcile_snapshot_carries_focus_and_cursor_modes() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let terminal_uuid = uuid::Uuid::new_v4().to_string();
        let mut state =
            window_state(vec![managed_session("workspace-1", "Workspace", term(&terminal_uuid))]);

        let mut snap = pane_snapshot(&terminal_uuid, "htop", "/home", b"");
        snap.terminal_modes = Some(v3::TerminalModeState {
            focus_reporting: true,
            cursor_hidden: true,
            alternate_screen: true,
            ..Default::default()
        });

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "workspace-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(&runtime_id, vec![snap]),
        });

        let modes =
            transition.pane_snapshot_restores[0].terminal_modes.as_ref().expect("modes present");
        assert!(modes.focus_reporting);
        assert!(modes.cursor_hidden);
        assert!(modes.alternate_screen);
    }

    #[test]
    fn reconcile_endpoint_event_pane_created_updates_binding_and_requests_recovery() {
        let mut state =
            window_state(vec![managed_session("workspace-1", "Workspace", term("pane-1"))]);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::PaneCreated {
            workspace_id: "workspace-1".into(),
            layout_terminal_uuid: "pane-1".into(),
            runtime_id: "d7d04564-b2bf-4302-9495-e65c4df12ac6".into(),
            runtime_pane_id: "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26".into(),
        });

        assert_eq!(transition.connected_layout_terminals, vec!["pane-1".to_string()]);
        assert_eq!(transition.layout_terminals_to_recover, vec!["pane-1".to_string()]);
        assert_eq!(
            transition.connection_status_updates,
            vec![ConnectionStatusUpdate {
                workspace_id: "workspace-1".into(),
                status: ConnectionStatus::Connected,
            }],
        );
        assert_eq!(
            state.workspaces[0].runtime.pane_bindings.get("pane-1").map(String::as_str),
            Some("598b80fe-b96b-4fbf-8e2d-f2610b6f4f26"),
        );
    }

    #[test]
    fn reconcile_endpoint_event_pane_closed_removes_terminal_and_rebuilds_workspace() {
        let mut session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            hsplit(term("left"), term("right")),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
        session.runtime.bind_runtime_pane("left", "07fa83b4-9ae3-4354-a1c5-1f685ffab370");
        session.runtime.bind_runtime_pane("right", "0d88f17f-626d-40b8-a1d3-6a42af628ac9");
        let mut state = window_state(vec![session]);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::PaneClosed {
            workspace_id: "workspace-1".into(),
            layout_terminal_uuid: "right".into(),
            runtime_id: "d7d04564-b2bf-4302-9495-e65c4df12ac6".into(),
            runtime_pane_id: "0d88f17f-626d-40b8-a1d3-6a42af628ac9".into(),
        });

        assert_eq!(transition.removed_layout_terminals, vec!["right".to_string()]);
        assert_eq!(transition.rebuilt_workspaces.len(), 1);
        assert_eq!(state.workspaces[0].layout.terminal_uuids(), vec!["left".to_string()]);
    }

    #[test]
    fn reconcile_endpoint_event_workspace_detached_preserves_runtime_id_and_marks_disconnected() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let mut state = window_state(vec![managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            term("pane-1"),
            RuntimeEndpoint::remote("builder.example"),
            WorkspacePolicy::Persistent,
            Some(&runtime_id),
        )]);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceDetached {
            workspace_id: "workspace-1".into(),
            runtime_id: runtime_id.clone(),
        });

        assert_eq!(state.workspaces[0].runtime.runtime_id.as_deref(), Some(runtime_id.as_str()));
        assert_eq!(
            transition.connection_status_updates,
            vec![ConnectionStatusUpdate {
                workspace_id: "workspace-1".into(),
                status: ConnectionStatus::Disconnected,
            }],
        );
    }

    #[test]
    fn reconcile_endpoint_event_runtime_terminated_clears_runtime_id_and_marks_disconnected() {
        let mut state = window_state(vec![managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            term("pane-1"),
            RuntimeEndpoint::remote("builder.example"),
            WorkspacePolicy::Persistent,
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        )]);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::RuntimeTerminated {
            workspace_id: "workspace-1".into(),
            runtime_id: "d7d04564-b2bf-4302-9495-e65c4df12ac6".into(),
            reason: v3::RuntimeTerminationReason::Explicit,
        });

        assert_eq!(state.workspaces[0].runtime.runtime_id, None);
        assert_eq!(
            transition.connection_status_updates,
            vec![ConnectionStatusUpdate {
                workspace_id: "workspace-1".into(),
                status: ConnectionStatus::Disconnected,
            }],
        );
    }

    /// After the layout/state/recovery module split, session types imported
    /// through `crate::workspace::*` must still compose correctly in workspace
    /// state operations.
    #[test]
    fn session_types_compose_after_module_split() {
        let mut state = window_state(vec![workspace("s1", "Work", term("t1"))]);
        let ws = &mut state.workspaces[0];
        ws.set_recovery("t1", PaneRecovery::empty_shell());
        assert!(ws.recovery_for("t1").is_some());
        ws.prune_recovery();
        assert!(ws.recovery_for("t1").is_some());
    }

    #[test]
    fn dismissed_runtime_is_not_resurrected_by_inventory() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let pane_id = uuid::Uuid::new_v4().to_string();
        let mut state = WindowState::default_for_test();

        // Dismiss the runtime (simulates user closing the workspace).
        state.dismiss_runtime(&RuntimeEndpoint::Local, &runtime_id);

        // Inventory refresh reports the runtime still exists on the daemon.
        let transition = state.reconcile_endpoint_event(&EndpointEvent::InventoryLoaded {
            endpoint: RuntimeEndpoint::Local,
            runtimes: vec![rt_info(
                &runtime_id,
                "Should Not Resurrect",
                v3::RuntimePolicy::Persistent,
                vec![pane_info(&pane_id, "Shell", "/tmp")],
                Some(&pane_id),
            )],
        });

        assert!(
            transition.recovered_workspaces.is_empty(),
            "dismissed runtime must not be resurrected by inventory"
        );
        assert!(
            !state.workspaces.iter().any(|s| s.runtime.runtime_id.as_deref() == Some(&runtime_id)),
            "dismissed runtime must not appear in session state"
        );
    }

    /// Closing a remote workspace must prevent resurrection from the remote
    /// daemon's inventory. Regression test for #248.
    #[test]
    fn dismissed_remote_runtime_is_not_resurrected_by_inventory() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let pane_id = uuid::Uuid::new_v4().to_string();
        let endpoint = RuntimeEndpoint::remote("build-host");
        let mut state = WindowState::default_for_test();

        state.dismiss_runtime(&endpoint, &runtime_id);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::InventoryLoaded {
            endpoint: endpoint.clone(),
            runtimes: vec![rt_info(
                &runtime_id,
                "Remote Work",
                v3::RuntimePolicy::Persistent,
                vec![pane_info(&pane_id, "bash", "/home/user")],
                Some(&pane_id),
            )],
        });

        assert!(
            transition.recovered_workspaces.is_empty(),
            "dismissed remote runtime must not be resurrected"
        );
    }

    #[test]
    fn dismissed_runtime_ids_survive_serde_roundtrip() {
        let mut state = WindowState::default_for_test();
        state.dismiss_runtime(&RuntimeEndpoint::Local, "runtime-abc");
        state.dismiss_runtime(&RuntimeEndpoint::Local, "runtime-def");

        let json = serde_json::to_string(&state).unwrap();
        let restored: WindowState = serde_json::from_str(&json).unwrap();

        assert!(restored.dismissed_runtime_ids.contains("runtime-abc"));
        assert!(restored.dismissed_runtime_ids.contains("runtime-def"));
    }

    #[test]
    fn repeated_close_and_inventory_cycles_stay_clean() {
        let mut state = WindowState::default_for_test();

        for i in 0..5 {
            let runtime_id = uuid::Uuid::new_v4().to_string();
            let pane_id = uuid::Uuid::new_v4().to_string();

            state.dismiss_runtime(&RuntimeEndpoint::Local, &runtime_id);

            let transition = state.reconcile_endpoint_event(&EndpointEvent::InventoryLoaded {
                endpoint: RuntimeEndpoint::Local,
                runtimes: vec![rt_info(
                    &runtime_id,
                    &format!("Dismissed {i}"),
                    v3::RuntimePolicy::Persistent,
                    vec![pane_info(&pane_id, "bash", "/tmp")],
                    Some(&pane_id),
                )],
            });

            assert!(
                transition.recovered_workspaces.is_empty(),
                "cycle {i}: dismissed runtime must not be recovered"
            );
        }

        assert_eq!(
            state.dismissed_runtime_ids.len(),
            1,
            "only the last dismissed ID should survive — earlier ones were pruned by inventory reconciliation"
        );
    }

    #[test]
    fn close_workspace_without_runtime_id_removes_cleanly() {
        let mut state = window_state(vec![managed_session("ws-1", "Disconnected", term("t1"))]);
        // No runtime_id — workspace was never connected.
        state.workspaces[0].runtime.runtime_id = None;

        // Dismiss with empty string (no runtime to track).
        assert_eq!(state.workspaces.len(), 1);
        state.workspaces.retain(|s| s.uuid != "ws-1");
        assert!(state.workspaces.is_empty());
    }

    #[test]
    fn workspace_opened_preserves_layout_cwd_from_snapshot() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let pane_id = uuid::Uuid::new_v4().to_string();
        let mut state = window_state(vec![managed_session_with_runtime(
            "ws-1",
            "Work",
            term("t1"),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(&runtime_id),
        )]);
        state.workspaces[0].layout.set_terminal_cwd("t1", Some("/old/path".into()));

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: v3::RuntimeSnapshot {
                tree: None,
                default_active_pane_id: Vec::new(),
                runtime_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(&runtime_id).unwrap()),
                panes: vec![v3::PaneSnapshot {
                    pane_id: rttx_proto::uuid_to_bytes(uuid::Uuid::parse_str(&pane_id).unwrap()),
                    pane_output_seq: 0,
                    title: "bash".into(),
                    cwd: "/new/project".into(),
                    cols: 80,
                    rows: 24,
                    exit_status: None,
                    terminal_modes: None,
                    scrollback_tail: bytes::Bytes::new(),
                    total_scrollback_bytes: 0,
                    scrollback_complete: true,
                }],
                runtime_revision: 2,
                client_role: v3::RuntimeClientRole::Writer as i32,
            },
        });

        assert!(!transition.pane_snapshot_restores.is_empty());
        assert_eq!(transition.pane_snapshot_restores[0].cwd, "/new/project");

        // The layout CWD must also be updated from the snapshot.
        let session = state.workspaces.iter().find(|s| s.uuid == "ws-1").unwrap();
        let layout_uuid = &transition.pane_snapshot_restores[0].layout_terminal_uuid;
        assert_eq!(
            session.layout.terminal_cwd(layout_uuid).as_deref(),
            Some("/new/project"),
            "layout CWD must be updated from snapshot during workspace opened"
        );
    }

    /// Pane create requests must carry the layout node's CWD. #297.
    #[test]
    fn reconcile_workspace_opened_propagates_layout_cwd_to_pane_create_request() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let existing_uuid = uuid::Uuid::new_v4().to_string();
        let new_uuid = uuid::Uuid::new_v4().to_string();
        let mut state = window_state(vec![managed_session(
            "ws-1",
            "Workspace",
            hsplit(term(&existing_uuid), term_full(&new_uuid, "/srv/project", "Shell")),
        )]);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(
                &runtime_id,
                vec![pane_snapshot(&existing_uuid, "Shell", "/home", b"")],
            ),
        });

        assert_eq!(transition.pane_create_requests.len(), 1);
        assert_eq!(
            transition.pane_create_requests[0].cwd.as_deref(),
            Some("/srv/project"),
            "pane create request must carry layout CWD"
        );
    }

    /// When a workspace has `runtime_id` but only placeholder bindings (e.g.
    /// state saved before `PaneCreated` events arrived), reattaching to a
    /// runtime with existing panes must bind disconnected layout terminals
    /// to unclaimed runtime panes by position instead of leaving them blank.
    #[test]
    fn placeholder_bindings_reattach_matches_layout_to_runtime_panes_by_position() {
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let runtime_pane_a = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
        let runtime_pane_b = "0d88f17f-626d-40b8-a1d3-6a42af628ac9";

        // Workspace has runtime_id but only placeholder bindings (left→left, right→right).
        let session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            hsplit(term("left"), term("right")),
            RuntimeEndpoint::remote("cdt2"),
            WorkspacePolicy::Persistent,
            Some(runtime_id),
        );
        // managed_session_with_runtime already calls ensure_placeholder_bindings,
        // so pane_bindings = { "left": "left", "right": "right" }.
        assert_eq!(session.runtime.pane_bindings.get("left").map(String::as_str), Some("left"));
        assert_eq!(session.runtime.pane_bindings.get("right").map(String::as_str), Some("right"));

        let mut state = window_state(vec![session]);
        let snap = snapshot(
            runtime_id,
            vec![
                pane_snapshot(runtime_pane_a, "Shell", "/home", b"$ ls"),
                pane_snapshot(runtime_pane_b, "Logs", "/var/log", b"tail"),
            ],
        );

        let opened = state
            .apply_managed_workspace_opened("workspace-1", runtime_id, &snap)
            .expect("workspace open should succeed");

        // Both layout terminals should be bound to runtime panes.
        assert_eq!(
            opened.session_state.runtime.pane_bindings.get("left").map(String::as_str),
            Some(runtime_pane_a),
            "first layout terminal should bind to first runtime pane by position",
        );
        assert_eq!(
            opened.session_state.runtime.pane_bindings.get("right").map(String::as_str),
            Some(runtime_pane_b),
            "second layout terminal should bind to second runtime pane by position",
        );

        // No new panes should be created — the runtime already has matching panes.
        assert!(
            opened.panes_to_create.is_empty(),
            "should not create new panes when runtime already has panes for each layout terminal",
        );

        // No runtime panes should be skipped or recovered into new layout terminals.
        assert!(
            opened.skipped_runtime_panes.is_empty(),
            "all runtime panes should be claimed by layout terminals",
        );

        // Snapshot restores should be emitted for both panes.
        assert_eq!(opened.snapshot_restores.len(), 2);
    }

    /// When placeholder-only bindings reattach to a runtime with fewer panes
    /// than layout terminals, the excess layout terminals must request new
    /// daemon panes so they don't stay blank.
    #[test]
    fn placeholder_bindings_reattach_creates_panes_for_excess_layout_terminals() {
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let runtime_pane_a = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";

        // 2 layout terminals, but runtime only has 1 pane.
        let session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            hsplit(term("left"), term("right")),
            RuntimeEndpoint::remote("cdt2"),
            WorkspacePolicy::Persistent,
            Some(runtime_id),
        );
        let mut state = window_state(vec![session]);
        let snap =
            snapshot(runtime_id, vec![pane_snapshot(runtime_pane_a, "Shell", "/home", b"$ ls")]);

        let opened = state
            .apply_managed_workspace_opened("workspace-1", runtime_id, &snap)
            .expect("workspace open should succeed");

        // First layout terminal should bind to the runtime pane.
        assert_eq!(
            opened.session_state.runtime.pane_bindings.get("left").map(String::as_str),
            Some(runtime_pane_a),
        );

        // Second layout terminal should request a new pane.
        assert_eq!(opened.panes_to_create, vec!["right".to_string()]);
    }

    #[test]
    fn snapshot_restore_carries_daemon_pane_dimensions() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let terminal_uuid = uuid::Uuid::new_v4().to_string();
        let mut state =
            window_state(vec![managed_session("ws-1", "Workspace", term(&terminal_uuid))]);

        let mut snap = pane_snapshot(&terminal_uuid, "Shell", "/home", b"$ ");
        snap.cols = 200;
        snap.rows = 50;

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(&runtime_id, vec![snap]),
        });

        assert_eq!(transition.pane_snapshot_restores.len(), 1);
        let restore = &transition.pane_snapshot_restores[0];
        assert_eq!(restore.cols, 200);
        assert_eq!(restore.rows, 50);
    }

    #[test]
    fn input_sync_targets_returns_siblings_when_enabled() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let mut session = managed_session_with_runtime(
            "ws-1",
            "Workspace",
            hsplit(term("pane-1"), term("pane-2")),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(&runtime_id),
        );
        session.input_sync = true;
        session.runtime.bind_runtime_pane("pane-1", "daemon-pane-1");
        session.runtime.bind_runtime_pane("pane-2", "daemon-pane-2");
        let state = window_state(vec![session]);

        let targets = state.input_sync_targets("pane-1");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].runtime_pane_id, "daemon-pane-2");
    }

    #[test]
    fn input_sync_targets_empty_when_disabled() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let mut session = managed_session_with_runtime(
            "ws-1",
            "Workspace",
            hsplit(term("pane-1"), term("pane-2")),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(&runtime_id),
        );
        session.runtime.bind_runtime_pane("pane-1", "daemon-pane-1");
        session.runtime.bind_runtime_pane("pane-2", "daemon-pane-2");
        let state = window_state(vec![session]);

        assert!(state.input_sync_targets("pane-1").is_empty());
    }

    #[test]
    fn input_sync_targets_skips_pending_panes() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let mut session = managed_session_with_runtime(
            "ws-1",
            "Workspace",
            hsplit(term("pane-1"), term("pane-2")),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(&runtime_id),
        );
        session.input_sync = true;
        session.runtime.bind_runtime_pane("pane-1", "daemon-pane-1");
        // pane-2 stays pending (placeholder binding only)
        let state = window_state(vec![session]);

        assert!(state.input_sync_targets("pane-1").is_empty());
    }

    #[test]
    fn input_sync_targets_empty_for_direct_workspace() {
        let mut session = workspace("s1", "Direct", hsplit(term("pane-1"), term("pane-2")));
        session.input_sync = true;
        let state = window_state(vec![session]);

        assert!(state.input_sync_targets("pane-1").is_empty());
    }

    /// Regression test for #547: repeated reconnect cycles must not grow the
    /// layout. When bindings become stale (e.g. daemon restart assigns new
    /// pane IDs), disconnected layout terminals should be matched to
    /// unclaimed runtime panes by position instead of creating new splits.
    #[test]
    fn repeated_reconnect_cycles_do_not_grow_layout() {
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";

        let session = managed_session_with_runtime(
            "workspace-1",
            "Home",
            hsplit(term("left"), term("right")),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(runtime_id),
        );
        let mut state = window_state(vec![session]);

        // Simulate 5 reconnect cycles, each with fresh daemon pane IDs
        // (as if the daemon restarted between cycles).
        for cycle in 0..5 {
            let pane_a = uuid::Uuid::new_v4().to_string();
            let pane_b = uuid::Uuid::new_v4().to_string();
            let snap = snapshot(
                runtime_id,
                vec![
                    pane_snapshot(&pane_a, "Shell", "/home", b"$ ls"),
                    pane_snapshot(&pane_b, "Logs", "/var/log", b"tail"),
                ],
            );

            let opened = state
                .apply_managed_workspace_opened("workspace-1", runtime_id, &snap)
                .expect("workspace open should succeed");

            assert_eq!(
                opened.session_state.layout.terminal_count(),
                2,
                "cycle {cycle}: layout must stay at 2 terminals",
            );
            assert!(
                opened.skipped_runtime_panes.is_empty(),
                "cycle {cycle}: no runtime panes should be skipped",
            );
        }
    }

    /// When stale bindings can't match current runtime panes, disconnected
    /// layout terminals should be positionally matched to unclaimed runtime
    /// panes — not left blank while new splits are created.
    #[test]
    fn stale_bindings_reconnect_matches_by_position() {
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let old_pane_a = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
        let old_pane_b = "0d88f17f-626d-40b8-a1d3-6a42af628ac9";
        let new_pane_a = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let new_pane_b = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";

        // Workspace has real (non-placeholder) bindings from a previous connection.
        let mut session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            hsplit(term("left"), term("right")),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(runtime_id),
        );
        session.runtime.bind_runtime_pane("left", old_pane_a);
        session.runtime.bind_runtime_pane("right", old_pane_b);
        let mut state = window_state(vec![session]);

        // Daemon restarted — new pane IDs.
        let snap = snapshot(
            runtime_id,
            vec![
                pane_snapshot(new_pane_a, "Shell", "/home", b"$ ls"),
                pane_snapshot(new_pane_b, "Logs", "/var/log", b"tail"),
            ],
        );

        let opened = state
            .apply_managed_workspace_opened("workspace-1", runtime_id, &snap)
            .expect("workspace open should succeed");

        assert_eq!(
            opened.session_state.layout.terminal_count(),
            2,
            "layout must not grow when daemon pane count matches layout terminal count",
        );
        assert_eq!(
            opened.session_state.runtime.pane_bindings.get("left").map(String::as_str),
            Some(new_pane_a),
        );
        assert_eq!(
            opened.session_state.runtime.pane_bindings.get("right").map(String::as_str),
            Some(new_pane_b),
        );
        assert!(opened.skipped_runtime_panes.is_empty());
    }

    #[test]
    fn reconcile_session_missing_emits_status_update() {
        let mut state = WindowState::default_for_test();

        let transition =
            state.reconcile_endpoint_event(&EndpointEvent::WorkspaceConnectionChanged {
                workspace_id: "workspace-1".into(),
                status: ConnectionStatus::SessionMissing,
            });

        assert_eq!(
            transition.connection_status_updates,
            vec![ConnectionStatusUpdate {
                workspace_id: "workspace-1".into(),
                status: ConnectionStatus::SessionMissing,
            }],
        );
    }

    #[test]
    fn workspace_opened_returns_previous_layout_terminals() {
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let first_runtime_pane = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
        let mut session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            hsplit(term("left"), term("right")),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(runtime_id),
        );
        session.runtime.bind_runtime_pane("left", first_runtime_pane);
        let mut state = window_state(vec![session]);

        let opened = state
            .apply_managed_workspace_opened(
                "workspace-1",
                runtime_id,
                &snapshot(
                    runtime_id,
                    vec![pane_snapshot(first_runtime_pane, "Shell", "/srv", b"")],
                ),
            )
            .expect("should reconcile");

        assert_eq!(
            opened.previous_layout_terminals,
            vec!["left".to_string(), "right".to_string()],
            "previous_layout_terminals should capture the pre-reconciliation UUIDs",
        );
    }

    #[test]
    fn reconcile_workspace_opened_emits_removed_for_stale_terminals() {
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let runtime_pane = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
        let mut session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            hsplit(term("left"), term("right")),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some(runtime_id),
        );
        session.runtime.bind_runtime_pane("left", runtime_pane);
        let mut state = window_state(vec![session]);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "workspace-1".into(),
            runtime_id: runtime_id.into(),
            snapshot: snapshot(runtime_id, vec![pane_snapshot(runtime_pane, "Shell", "/srv", b"")]),
        });

        // Both "left" and "right" were in the previous layout. After
        // reconciliation, "left" is matched to runtime_pane and "right"
        // stays disconnected but remains in the layout (it gets a
        // pane_create_request). So no terminals are removed.
        assert!(
            !transition.removed_layout_terminals.contains(&"left".to_string()),
            "matched terminal should not be marked as removed",
        );
        assert!(
            !transition.removed_layout_terminals.contains(&"right".to_string()),
            "disconnected terminal stays in layout and should not be removed",
        );
        assert_eq!(
            transition.rebuilt_workspaces.len(),
            1,
            "workspace should be rebuilt after reconciliation",
        );
        assert_eq!(
            transition.rebuilt_workspaces[0].session_state.layout.terminal_uuids(),
            vec!["left".to_string(), "right".to_string()],
            "both terminals should remain in the reconciled layout",
        );
    }

    #[test]
    fn dismissed_runtime_ids_pruned_after_inventory_reconciliation() {
        let mut state = WindowState::default_for_test();
        let stale_id = uuid::Uuid::new_v4().to_string();
        let live_id = uuid::Uuid::new_v4().to_string();
        let pane_id = uuid::Uuid::new_v4().to_string();

        state.dismiss_runtime(&RuntimeEndpoint::Local, &stale_id);
        state.dismiss_runtime(&RuntimeEndpoint::Local, &live_id);
        assert_eq!(state.dismissed_runtime_ids.len(), 2);

        // Inventory only contains live_id — stale_id was already removed by daemon.
        let _transition = state.reconcile_endpoint_event(&EndpointEvent::InventoryLoaded {
            endpoint: RuntimeEndpoint::Local,
            runtimes: vec![rt_info(
                &live_id,
                "Still Running",
                v3::RuntimePolicy::Persistent,
                vec![pane_info(&pane_id, "bash", "/tmp")],
                Some(&pane_id),
            )],
        });

        assert!(
            !state.dismissed_runtime_ids.contains(&stale_id),
            "stale dismissed ID should be pruned after inventory reconciliation"
        );
        assert!(
            state.dismissed_runtime_ids.contains(&live_id),
            "live dismissed ID should be retained"
        );
    }

    // ── Resync (StreamOverflow recovery) ──

    #[test]
    fn resync_produces_snapshot_restores_for_bound_panes() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let pane_uuid = uuid::Uuid::new_v4().to_string();
        let mut state = window_state(vec![managed_session("ws-1", "Workspace", term(&pane_uuid))]);
        // Simulate an opened workspace so bindings exist.
        let _ = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(
                &runtime_id,
                vec![pane_snapshot(&pane_uuid, "bash", "/home", b"initial")],
            ),
        });

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceResynced {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(
                &runtime_id,
                vec![pane_snapshot(&pane_uuid, "bash", "/home/project", b"resynced output")],
            ),
        });

        assert_eq!(transition.pane_snapshot_restores.len(), 1);
        let restore = &transition.pane_snapshot_restores[0];
        assert_eq!(restore.layout_terminal_uuid, pane_uuid);
        assert_eq!(restore.cwd, "/home/project");
        assert_eq!(restore.scrollback_tail, bytes::Bytes::from_static(b"resynced output"));
    }

    #[test]
    fn resync_ignores_unknown_workspace() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let mut state = window_state(vec![]);

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceResynced {
            workspace_id: "nonexistent".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(&runtime_id, vec![]),
        });

        assert!(transition.pane_snapshot_restores.is_empty());
    }

    #[test]
    fn resync_skips_unbound_runtime_panes() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let pane_uuid = uuid::Uuid::new_v4().to_string();
        let extra_pane = uuid::Uuid::new_v4().to_string();
        let mut state = window_state(vec![managed_session("ws-1", "Workspace", term(&pane_uuid))]);
        let _ = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(&runtime_id, vec![pane_snapshot(&pane_uuid, "bash", "/home", b"")]),
        });

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceResynced {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(
                &runtime_id,
                vec![
                    pane_snapshot(&pane_uuid, "bash", "/home", b"resynced"),
                    pane_snapshot(&extra_pane, "zsh", "/tmp", b"extra"),
                ],
            ),
        });

        assert_eq!(transition.pane_snapshot_restores.len(), 1);
        assert_eq!(transition.pane_snapshot_restores[0].layout_terminal_uuid, pane_uuid);
    }

    #[test]
    fn resync_does_not_rebuild_layout() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let pane_uuid = uuid::Uuid::new_v4().to_string();
        let mut state = window_state(vec![managed_session("ws-1", "Workspace", term(&pane_uuid))]);
        let _ = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(&runtime_id, vec![pane_snapshot(&pane_uuid, "bash", "/home", b"")]),
        });

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceResynced {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(
                &runtime_id,
                vec![pane_snapshot(&pane_uuid, "bash", "/home", b"resynced")],
            ),
        });

        assert!(transition.rebuilt_workspaces.is_empty());
        assert!(transition.recovered_workspaces.is_empty());
        assert!(transition.pane_create_requests.is_empty());
    }

    #[test]
    fn resync_carries_terminal_modes() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let pane_uuid = uuid::Uuid::new_v4().to_string();
        let mut state = window_state(vec![managed_session("ws-1", "Workspace", term(&pane_uuid))]);
        let _ = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceOpened {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(&runtime_id, vec![pane_snapshot(&pane_uuid, "bash", "/home", b"")]),
        });

        let mut snap = pane_snapshot(&pane_uuid, "vim", "/home", b"vim content");
        snap.terminal_modes = Some(v3::TerminalModeState {
            bracketed_paste: true,
            application_cursor_keys: true,
            ..Default::default()
        });

        let transition = state.reconcile_endpoint_event(&EndpointEvent::WorkspaceResynced {
            workspace_id: "ws-1".into(),
            runtime_id: runtime_id.clone(),
            snapshot: snapshot(&runtime_id, vec![snap]),
        });

        assert_eq!(transition.pane_snapshot_restores.len(), 1);
        let modes = transition.pane_snapshot_restores[0]
            .terminal_modes
            .as_ref()
            .expect("modes should be present");
        assert!(modes.bracketed_paste);
        assert!(modes.application_cursor_keys);
    }

    #[test]
    fn store_preferences_conversion_preserves_all_fields() {
        use crate::store::models::preferences::PreferencesV1;

        let prefs = crate::preferences::Preferences {
            font: "Hack 11".into(),
            scrollback_lines: 5000,
            smart_clipboard: true,
            paste_guard_threshold: 512,
            ..Default::default()
        };
        let v1: PreferencesV1 = (&prefs).into();
        let round_tripped: crate::preferences::Preferences = v1.into();
        assert_eq!(round_tripped.font, "Hack 11");
        assert_eq!(round_tripped.scrollback_lines, 5000);
        assert!(round_tripped.smart_clipboard);
        assert_eq!(round_tripped.paste_guard_threshold, 512);
    }

    // ── Reverse index tests ──────────────────────────────────────

    #[test]
    fn reverse_index_matches_linear_scan_for_bound_pane() {
        let endpoint = RuntimeEndpoint::remote("builder.example");
        let runtime_pane_id = "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26";
        let mut session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            term("pane-1"),
            endpoint.clone(),
            WorkspacePolicy::Persistent,
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
        session.runtime.bind_runtime_pane("pane-1", runtime_pane_id);
        let state = window_state(vec![session]);

        assert_eq!(
            state.runtime_pane_target(&endpoint, runtime_pane_id),
            Some(("workspace-1".into(), "pane-1".into())),
        );
    }

    #[test]
    fn reverse_index_excludes_pending_placeholder_bindings() {
        let state = window_state(vec![managed_session("workspace-1", "Workspace", term("pane-1"))]);

        // Placeholder bindings (self-bindings) are pending — not in the index.
        assert!(
            state.runtime_pane_target(&RuntimeEndpoint::Local, "pane-1").is_none(),
            "pending placeholder bindings must not appear in the reverse index",
        );
    }

    #[test]
    fn reverse_index_consistent_after_pane_create() {
        let mut state =
            window_state(vec![managed_session("workspace-1", "Workspace", term("pane-1"))]);

        state.apply_managed_pane_created(
            "workspace-1",
            "pane-1",
            "d7d04564-b2bf-4302-9495-e65c4df12ac6",
            "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26",
        );

        assert_eq!(
            state.runtime_pane_target(
                &RuntimeEndpoint::Local,
                "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26"
            ),
            Some(("workspace-1".into(), "pane-1".into())),
        );
    }

    #[test]
    fn reverse_index_consistent_after_pane_close() {
        let mut session = managed_session_with_runtime(
            "workspace-1",
            "Workspace",
            hsplit(term("left"), term("right")),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
        session.runtime.bind_runtime_pane("left", "07fa83b4-9ae3-4354-a1c5-1f685ffab370");
        session.runtime.bind_runtime_pane("right", "0d88f17f-626d-40b8-a1d3-6a42af628ac9");
        let mut state = window_state(vec![session]);

        state.apply_managed_pane_closed("workspace-1", "left");

        assert!(
            state
                .runtime_pane_target(
                    &RuntimeEndpoint::Local,
                    "07fa83b4-9ae3-4354-a1c5-1f685ffab370"
                )
                .is_none(),
            "closed pane must be removed from the reverse index",
        );
        assert_eq!(
            state.runtime_pane_target(
                &RuntimeEndpoint::Local,
                "0d88f17f-626d-40b8-a1d3-6a42af628ac9"
            ),
            Some(("workspace-1".into(), "right".into())),
        );
    }

    #[test]
    fn reverse_index_consistent_after_workspace_opened_reconciliation() {
        let runtime_id = "d7d04564-b2bf-4302-9495-e65c4df12ac6";
        let runtime_pane_a = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
        let runtime_pane_b = "0d88f17f-626d-40b8-a1d3-6a42af628ac9";
        let mut state = window_state(vec![managed_session(
            "workspace-1",
            "Workspace",
            hsplit(term("left"), term("right")),
        )]);

        state.apply_managed_workspace_opened(
            "workspace-1",
            runtime_id,
            &snapshot(
                runtime_id,
                vec![
                    pane_snapshot(runtime_pane_a, "Shell", "/home", b""),
                    pane_snapshot(runtime_pane_b, "Logs", "/var", b""),
                ],
            ),
        );

        assert_eq!(
            state.runtime_pane_target(&RuntimeEndpoint::Local, runtime_pane_a),
            Some(("workspace-1".into(), "left".into())),
        );
        assert_eq!(
            state.runtime_pane_target(&RuntimeEndpoint::Local, runtime_pane_b),
            Some(("workspace-1".into(), "right".into())),
        );
    }

    #[test]
    fn reverse_index_consistent_after_inventory_recovery() {
        let runtime_id = uuid::Uuid::new_v4().to_string();
        let pane_id = uuid::Uuid::new_v4().to_string();
        let mut state = WindowState::default_for_test();

        state.recover_managed_workspaces_from_inventory(
            &RuntimeEndpoint::Local,
            &[rt_info(
                &runtime_id,
                "Recovered",
                v3::RuntimePolicy::Persistent,
                vec![pane_info(&pane_id, "Shell", "/home")],
                Some(&pane_id),
            )],
        );

        // Inventory recovery creates self-bindings that are NOT pending
        // (the pane IDs come from the daemon), so they should be in the index.
        assert_eq!(
            state.runtime_pane_target(&RuntimeEndpoint::Local, &pane_id),
            Some((format!("inventory:local:{runtime_id}"), pane_id.clone())),
        );
    }

    #[test]
    fn reverse_index_not_serialized() {
        let mut session = managed_session_with_runtime(
            "ws-1",
            "Work",
            term("t1"),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
        session.runtime.bind_runtime_pane("t1", "598b80fe-b96b-4fbf-8e2d-f2610b6f4f26");
        let state = window_state(vec![session]);

        let json = serde_json::to_string(&state).unwrap();
        assert!(
            !json.contains("pane_reverse_index"),
            "reverse index must not appear in serialized JSON",
        );

        let deserialized: WindowState = serde_json::from_str(&json).unwrap();
        assert!(
            deserialized.pane_reverse_index.is_empty(),
            "deserialized state must have empty reverse index",
        );
    }

    #[test]
    fn reverse_index_multi_workspace_multi_endpoint() {
        let local_pane = "07fa83b4-9ae3-4354-a1c5-1f685ffab370";
        let remote_pane = "0d88f17f-626d-40b8-a1d3-6a42af628ac9";
        let remote_endpoint = RuntimeEndpoint::remote("builder.example");

        let mut local_session = managed_session_with_runtime(
            "ws-local",
            "Local",
            term("local-t1"),
            RuntimeEndpoint::Local,
            WorkspacePolicy::Persistent,
            Some("d7d04564-b2bf-4302-9495-e65c4df12ac6"),
        );
        local_session.runtime.bind_runtime_pane("local-t1", local_pane);

        let mut remote_session = managed_session_with_runtime(
            "ws-remote",
            "Remote",
            term("remote-t1"),
            remote_endpoint.clone(),
            WorkspacePolicy::Persistent,
            Some("598b80fe-b96b-4fbf-8e2d-f2610b6f4f26"),
        );
        remote_session.runtime.bind_runtime_pane("remote-t1", remote_pane);

        let state = window_state(vec![local_session, remote_session]);

        assert_eq!(
            state.runtime_pane_target(&RuntimeEndpoint::Local, local_pane),
            Some(("ws-local".into(), "local-t1".into())),
        );
        assert_eq!(
            state.runtime_pane_target(&remote_endpoint, remote_pane),
            Some(("ws-remote".into(), "remote-t1".into())),
        );
        // Cross-endpoint lookup must not match.
        assert!(state.runtime_pane_target(&RuntimeEndpoint::Local, remote_pane).is_none());
        assert!(state.runtime_pane_target(&remote_endpoint, local_pane).is_none());
    }
}
